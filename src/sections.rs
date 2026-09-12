/// On-disk section identifier, preserving unknown producer-defined types.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum SectionKind {
    /// EWF1 ASCII section name.
    Ewf1(String),
    /// EWF2 numeric section identifier.
    Ewf2(u32),
}

/// A section's location in its original segment. Payloads are not retained.
/// These are the descriptors accepted during opening, not a fresh integrity check.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct SectionInfo {
    /// Zero-based segment index, resolving through `Image::segment_filename`.
    pub segment_index: usize,
    /// Original section identifier.
    pub kind: SectionKind,
    /// Descriptor byte offset within the segment.
    pub descriptor_offset: u64,
    /// Payload byte offset within the segment.
    pub data_offset: u64,
    /// Payload byte length, excluding descriptor and padding.
    pub data_size: u64,
    /// EWF1 next descriptor or EWF2 previous descriptor offset.
    pub linked_descriptor_offset: u64,
    /// Raw EWF2 data flags; zero for EWF1.
    pub data_flags: u32,
}
