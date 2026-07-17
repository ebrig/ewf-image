use crate::{EwfError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableRangeKind {
    Ewf1,
    Ewf2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TableRange {
    pub(crate) kind: TableRangeKind,
    pub(crate) segment_index: usize,
    pub(crate) first_chunk: u64,
    pub(crate) chunk_count: u64,
    pub(crate) entries_offset: u64,
    pub(crate) base_offset: u64,
    pub(crate) data_end: Option<u64>,
    pub(crate) ewf1_allow_large_compressed_chunks: bool,
    pub(crate) ewf2_compression_method: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LazyChunkIndex {
    logical_chunks: u64,
    ranges: Vec<TableRange>,
}

impl LazyChunkIndex {
    pub(crate) fn new(
        mut ranges: Vec<TableRange>,
        logical_size: u64,
        chunk_size: u64,
    ) -> Result<Self> {
        let logical_chunks = logical_chunk_count(logical_size, chunk_size)?;
        ranges.sort_by_key(|range| range.first_chunk);

        let mut expected_next = 0_u64;
        for range in &ranges {
            if range.chunk_count == 0 {
                return Err(EwfError::Malformed("table range has zero chunks".into()));
            }
            if range.first_chunk < expected_next {
                return Err(EwfError::Malformed(format!(
                    "table range starting at chunk {} overlaps previous range ending at chunk {}",
                    range.first_chunk, expected_next
                )));
            }
            if range.first_chunk > expected_next {
                return Err(EwfError::Malformed(format!(
                    "table range starts at chunk {}, expected {}",
                    range.first_chunk, expected_next
                )));
            }
            expected_next = range
                .first_chunk
                .checked_add(range.chunk_count)
                .ok_or_else(|| EwfError::Malformed("table range chunk count overflow".into()))?;
        }

        if expected_next != logical_chunks {
            return Err(EwfError::Malformed(format!(
                "table coverage ends at chunk {expected_next}, expected {logical_chunks}"
            )));
        }

        Ok(Self {
            logical_chunks,
            ranges,
        })
    }

    #[cfg(test)]
    pub(crate) fn logical_chunks(&self) -> u64 {
        self.logical_chunks
    }

    #[cfg(test)]
    pub(crate) fn range_for(&self, chunk_id: u64) -> Result<&TableRange> {
        self.range_index_for(chunk_id).map(|(_, range)| range)
    }

    pub(crate) fn range_index_for(&self, chunk_id: u64) -> Result<(usize, &TableRange)> {
        if chunk_id >= self.logical_chunks {
            return Err(EwfError::Malformed(format!(
                "chunk {chunk_id} is outside logical chunk count {}",
                self.logical_chunks
            )));
        }

        let index = self.ranges.partition_point(|range| {
            range.first_chunk.saturating_add(range.chunk_count) <= chunk_id
        });
        self.ranges
            .get(index)
            .filter(|range| chunk_id >= range.first_chunk)
            .map(|range| (index, range))
            .ok_or_else(|| EwfError::Malformed(format!("chunk {chunk_id} is not covered")))
    }

    #[cfg(test)]
    pub(crate) fn range_count(&self) -> usize {
        self.ranges.len()
    }
}

pub(crate) fn logical_chunk_count(logical_size: u64, chunk_size: u64) -> Result<u64> {
    if chunk_size == 0 {
        return Err(EwfError::Malformed("chunk size is zero".into()));
    }
    if logical_size == 0 {
        return Ok(0);
    }

    logical_size
        .checked_sub(1)
        .map(|value| value / chunk_size + 1)
        .ok_or_else(|| EwfError::Malformed("logical chunk count underflow".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(first_chunk: u64, chunk_count: u64) -> TableRange {
        TableRange {
            kind: TableRangeKind::Ewf1,
            segment_index: 0,
            first_chunk,
            chunk_count,
            entries_offset: 128,
            base_offset: 1024,
            data_end: Some(2048),
            ewf1_allow_large_compressed_chunks: false,
            ewf2_compression_method: None,
        }
    }

    #[test]
    fn logical_chunk_count_rounds_up() {
        assert_eq!(logical_chunk_count(0, 32_768).unwrap(), 0);
        assert_eq!(logical_chunk_count(1, 32_768).unwrap(), 1);
        assert_eq!(logical_chunk_count(32_768, 32_768).unwrap(), 1);
        assert_eq!(logical_chunk_count(32_769, 32_768).unwrap(), 2);
    }

    #[test]
    fn logical_chunk_count_rejects_zero_chunk_size() {
        let err = logical_chunk_count(100, 0).unwrap_err();

        assert!(matches!(err, EwfError::Malformed(_)));
    }

    #[test]
    fn lazy_index_accepts_exact_range_coverage() {
        let index =
            LazyChunkIndex::new(vec![range(0, 2), range(2, 3)], 5 * 32_768, 32_768).unwrap();

        assert_eq!(index.logical_chunks(), 5);
        assert_eq!(index.range_for(0).unwrap().first_chunk, 0);
        assert_eq!(index.range_for(4).unwrap().first_chunk, 2);
    }

    #[test]
    fn lazy_index_selects_the_correct_range_at_boundaries() {
        let index = LazyChunkIndex::new(
            vec![range(0, 2), range(2, 3), range(5, 1)],
            6 * 32_768,
            32_768,
        )
        .unwrap();

        assert_eq!(index.range_index_for(0).unwrap().0, 0);
        assert_eq!(index.range_index_for(1).unwrap().0, 0);
        assert_eq!(index.range_index_for(2).unwrap().0, 1);
        assert_eq!(index.range_index_for(4).unwrap().0, 1);
        assert_eq!(index.range_index_for(5).unwrap().0, 2);
    }

    #[test]
    fn lazy_index_rejects_gap_in_coverage() {
        let err =
            LazyChunkIndex::new(vec![range(0, 1), range(2, 1)], 3 * 32_768, 32_768).unwrap_err();

        assert!(matches!(err, EwfError::Malformed(_)));
    }

    #[test]
    fn lazy_index_rejects_overlapping_ranges() {
        let err =
            LazyChunkIndex::new(vec![range(0, 2), range(1, 2)], 3 * 32_768, 32_768).unwrap_err();

        assert!(matches!(err, EwfError::Malformed(_)));
    }

    #[test]
    fn lazy_index_rejects_missing_tail_coverage() {
        let err = LazyChunkIndex::new(vec![range(0, 1)], 2 * 32_768, 32_768).unwrap_err();

        assert!(matches!(err, EwfError::Malformed(_)));
    }

    #[test]
    fn lazy_index_handles_huge_images_without_eager_chunk_records() {
        let eight_tib = 8_u64 * 1024 * 1024 * 1024 * 1024;
        let chunks = logical_chunk_count(eight_tib, 32_768).unwrap();
        let index = LazyChunkIndex::new(vec![range(0, chunks)], eight_tib, 32_768).unwrap();

        assert_eq!(index.logical_chunks(), chunks);
        assert_eq!(index.range_count(), 1);
        assert_eq!(index.range_for(chunks - 1).unwrap().first_chunk, 0);
    }

    #[test]
    fn range_lookup_rejects_out_of_bounds_chunk() {
        let index = LazyChunkIndex::new(vec![range(0, 1)], 32_768, 32_768).unwrap();

        let err = index.range_for(1).unwrap_err();

        assert!(matches!(err, EwfError::Malformed(_)));
    }
}
