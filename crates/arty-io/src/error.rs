//! Errors and load warnings.

use std::fmt;
use std::path::PathBuf;

use arty_core::TreeError;

/// Why a load or save failed.
#[derive(Debug)]
pub enum IoError {
    Io { op: &'static str, source: std::io::Error },
    /// Not an `.arty` file at all.
    NotArty,
    /// Written by a newer ARTY with an incompatible major version.
    NewerFormat { major: u32 },
    /// A must-understand feature (CRITICAL section or required flag) this
    /// version does not know.
    UnsupportedFeature { tag: [u8; 4] },
    /// Damaged or hostile data. `offset` is the file offset of the
    /// structure that failed (best effort).
    Corrupt { what: &'static str, offset: u64 },
    LimitExceeded { what: &'static str, value: u64, limit: u64 },
    InvalidTree(TreeError),
    /// The file on disk changed since we last read or wrote it.
    ExternallyModified,
    /// Another process holds the file.
    Busy,
    /// Saved, but the final rename failed; the data is in this file.
    SavedToTemp(PathBuf),
    Cancelled,
}

impl IoError {
    pub fn corrupt(what: &'static str, offset: u64) -> Self {
        IoError::Corrupt { what, offset }
    }

    pub fn limit(what: &'static str, value: u64, limit: u64) -> Self {
        IoError::LimitExceeded { what, value, limit }
    }

    /// Adapter for `map_err`: tags an `std::io::Error` with the operation.
    pub fn io(op: &'static str) -> impl FnOnce(std::io::Error) -> IoError {
        move |source| IoError::Io { op, source }
    }
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoError::Io { op, source } => write!(f, "{op}: {source}"),
            IoError::NotArty => f.write_str("not an ARTY file"),
            IoError::NewerFormat { major } => {
                write!(f, "the file needs a newer ARTY (format version {major})")
            }
            IoError::UnsupportedFeature { tag } => {
                write!(f, "the file uses a feature this ARTY does not support ({})", tag_str(tag))
            }
            IoError::Corrupt { what, offset } => write!(f, "the file is damaged: {what} (at byte {offset})"),
            IoError::LimitExceeded { what, value, limit } => {
                write!(f, "the file is too large to open: {what} is {value}, the limit is {limit}")
            }
            IoError::InvalidTree(e) => write!(f, "invalid layer tree: {e:?}"),
            IoError::ExternallyModified => f.write_str("the file was changed by another program"),
            IoError::Busy => f.write_str("the file is in use by another program"),
            IoError::SavedToTemp(p) => {
                write!(f, "could not replace the file; your work was saved to {}", p.display())
            }
            IoError::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for IoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IoError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<TreeError> for IoError {
    fn from(e: TreeError) -> Self {
        IoError::InvalidTree(e)
    }
}

/// A problem a load recovered from. Shown to the user after opening.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadWarning {
    RecoveredTornTail,
    FellBackToCommit { seq: u64 },
    ClampedPixels { count: u64 },
    DamagedTiles { count: u32 },
    UnknownBlend { layer: u32, id: u8 },
    UnsupportedLayerKind { layer: u32, kind: u8 },
    SkippedSection { tag: [u8; 4] },
    ExtraPagesIgnored(u32),
    LossyName { layer: u32 },
    OpacityFixed { layer: u32 },
    FixedActiveLayer,
    FixedNextId,
    AddedMissingRaster,
    LegacyVectorRasterized { name: String },
    LegacyDroppedTiles { count: u32, reason: &'static str },
    LegacyBlendMapped { layer: u32, from: String },
    LegacyUnknownBlend { name: String },
    LegacyOrphan { layer: u32 },
    LegacyDuplicateRef { layer: u32 },
    LegacyDeepFolders { count: u32 },
    LegacyDefaultDpi,
}

impl fmt::Display for LoadWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadWarning::RecoveredTornTail => f.write_str("The end of the file was incomplete; the last complete save was loaded."),
            LoadWarning::FellBackToCommit { seq } => {
                write!(f, "The latest save was damaged; an earlier save (#{seq}) was loaded.")
            }
            LoadWarning::ClampedPixels { count } => write!(f, "{count} out-of-range pixel values were corrected."),
            LoadWarning::DamagedTiles { count } => write!(f, "{count} damaged tiles were left blank."),
            LoadWarning::UnknownBlend { layer, id } => {
                write!(f, "Layer {layer} uses an unknown blend mode ({id}); it was set to Normal.")
            }
            LoadWarning::UnsupportedLayerKind { layer, kind } => {
                write!(f, "Layer {layer} is of a kind this ARTY cannot edit ({kind}); it was loaded locked.")
            }
            LoadWarning::SkippedSection { tag } => write!(f, "Skipped unknown data ({}).", tag_str(tag)),
            LoadWarning::ExtraPagesIgnored(n) => write!(f, "{n} more pages were not loaded."),
            LoadWarning::LossyName { layer } => write!(f, "The name of layer {layer} was not valid text and was repaired."),
            LoadWarning::OpacityFixed { layer } => write!(f, "The opacity of layer {layer} was out of range and was fixed."),
            LoadWarning::FixedActiveLayer => f.write_str("The selected layer was missing; the top layer was selected."),
            LoadWarning::FixedNextId => f.write_str("Layer numbering was repaired."),
            LoadWarning::AddedMissingRaster => f.write_str("The file had no paintable layer; an empty one was added."),
            LoadWarning::LegacyVectorRasterized { name } => {
                write!(f, "Vector layer \"{name}\" was converted to pixels.")
            }
            LoadWarning::LegacyDroppedTiles { count, reason } => write!(f, "{count} tiles were dropped ({reason})."),
            LoadWarning::LegacyBlendMapped { layer, from } => {
                write!(f, "Layer {layer}: blend mode \"{from}\" was replaced by its closest match.")
            }
            LoadWarning::LegacyUnknownBlend { name } => {
                write!(f, "Unknown blend mode \"{name}\" was set to Normal.")
            }
            LoadWarning::LegacyOrphan { layer } => {
                write!(f, "Layer {layer} was not in the layer list and was placed at the top.")
            }
            LoadWarning::LegacyDuplicateRef { layer } => {
                write!(f, "Layer {layer} was listed more than once; extra entries were ignored.")
            }
            LoadWarning::LegacyDeepFolders { count } => write!(
                f,
                "{count} folders nested deeper than 3 levels were invisible in the old version and are now shown."
            ),
            LoadWarning::LegacyDefaultDpi => f.write_str("The old file had no resolution; the default was used."),
        }
    }
}

/// A 4-byte tag as text, with non-printable bytes escaped.
pub(crate) fn tag_str(tag: &[u8; 4]) -> String {
    tag.iter().flat_map(|b| std::ascii::escape_default(*b)).map(char::from).collect()
}
