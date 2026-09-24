//! Physical image replicas, equivalent map segmentation, and canonical stripes.
use aff4_image::DiskImageSet;
use std::io::{Read, Write};
use std::path::Path;

fn volume(path: &Path, name: &str, ranges: &[(u64, u64, u64)], block: u32) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    zip.set_comment(format!("aff4://{name}")).unwrap();
    let metadata = format!("@prefix a: <http://aff4.org/Schema#> .
        <aff4://disk> a a:DiskImage; a:blockSize {block}; a:dataStream <aff4://{name}/map> .
        <aff4://{name}/map> a a:Map; a:size 8 .
        <aff4://a/data> a a:ImageStream; a:stored <aff4://a>; a:size 8; a:chunkSize 8; a:chunksInSegment 1 .");
    let mut map = Vec::new();
    for &(start, length, offset) in ranges {
        map.extend_from_slice(&start.to_le_bytes());
        map.extend_from_slice(&length.to_le_bytes());
        map.extend_from_slice(&offset.to_le_bytes());
        map.extend_from_slice(&0u32.to_le_bytes());
    }
    let mut members = vec![
        ("version.txt", b"major=1\nminor=0\n".to_vec()),
        ("information.turtle", metadata.into_bytes()),
        ("map/map", map),
        ("map/idx", b"aff4://a/data\n".to_vec()),
    ];
    if name == "a" {
        members.push(("data/00000000", b"abcdefgh".to_vec()));
        members.push((
            "data/00000000.index",
            [0u64.to_le_bytes().as_slice(), &8u32.to_le_bytes()].concat(),
        ));
    }
    for (name, bytes) in members {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn replicated_disks_require_equivalent_ranges_and_geometry() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("a.aff4");
    let second = temp.path().join("b.aff4");
    volume(&first, "a", &[(0, 8, 0)], 512);
    volume(&second, "b", &[(0, 4, 0), (4, 4, 4)], 512);
    for input in [&first, &second] {
        let mut readers = DiskImageSet::discover(
            std::slice::from_ref(input),
            &[first.clone(), second.clone()],
        )
        .unwrap()
        .into_readers();
        assert_eq!(readers.len(), 1);
        assert_eq!(readers[0].info().image.volume_id, "aff4://a");
        assert_eq!(
            readers[0].info().reopen().unwrap().info(),
            readers[0].info()
        );
        let mut data = Vec::new();
        readers[0].read_to_end(&mut data).unwrap();
        assert_eq!(data, b"abcdefgh");
    }
    volume(&second, "b", &[(0, 4, 0), (4, 4, 0)], 512);
    assert!(
        DiskImageSet::discover(std::slice::from_ref(&first), std::slice::from_ref(&second))
            .is_err()
    );
    volume(&second, "b", &[(0, 8, 0)], 4096);
    assert!(DiskImageSet::discover(&[first], &[second]).is_err());
}

#[test]
#[ignore = "requires SHA256-pinned canonical stripe files"]
fn canonical_stripes_discover_one_disk_from_either_container() {
    use sha2::{Digest, Sha256};
    let paths = ["AFF4_STRIPE1", "AFF4_STRIPE2"]
        .map(|key| std::path::PathBuf::from(std::env::var_os(key).expect(key)));
    for (path, hash) in paths.iter().zip([
        "56fea0e0b4c94fb7ce780a39129054fe869ee2fcfa77ee7c6ede4830b035c8c8",
        "0d46baa88def85b784caf54a3a6c561e08019fbc22424a21f117d00b90c94505",
    ]) {
        assert_eq!(
            Sha256::digest(std::fs::read(path).unwrap())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            hash
        );
    }
    let mut identity = None;
    for path in &paths {
        let mut readers = DiskImageSet::discover(std::slice::from_ref(path), &paths)
            .unwrap()
            .into_readers();
        assert_eq!(readers.len(), 1);
        let reader = &mut readers[0];
        if let Some(previous) = &identity {
            assert_eq!(reader.info(), previous);
        }
        identity = Some(reader.info().clone());
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 1024 * 1024];
        loop {
            let count = reader.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        assert_eq!(
            hash.finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            "d7d6df4534f06568eb90a06e252592c9b79378b95bb9a7e01db3a388feda6c13"
        );
    }
}
