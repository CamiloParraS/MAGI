//! Optional search features (ADR-0010): which ones exist, where their
//! downloads live, and the per-file record of what a file was indexed without.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Search by meaning: e5 text embeddings.
    Meaning,
    /// Read text in images: OCR.
    ImageText,
    /// Find images by what they show: SigLIP 2.
    ImageVisual,
}

impl Feature {
    pub const ALL: [Feature; 3] = [Feature::Meaning, Feature::ImageText, Feature::ImageVisual];

    /// The `models/manifest.toml` slot holding this feature's download.
    pub fn slot(self) -> &'static str {
        match self {
            Self::Meaning => "text",
            Self::ImageText => "ocr",
            Self::ImageVisual => "image",
        }
    }

    /// This feature's bit in `files.features_missing`.
    pub fn bit(self) -> i64 {
        match self {
            Self::Meaning => 1,
            Self::ImageText => 2,
            Self::ImageVisual => 4,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Meaning => "meaning",
            Self::ImageText => "image_text",
            Self::ImageVisual => "image_visual",
        }
    }
}

impl std::str::FromStr for Feature {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|f| f.as_str() == s)
            .ok_or_else(|| Error::UnknownFeature(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_round_trip_through_their_wire_names() {
        for f in Feature::ALL {
            assert_eq!(f.as_str().parse::<Feature>().unwrap(), f);
        }
        assert!(matches!(
            "ocr".parse::<Feature>(),
            Err(crate::Error::UnknownFeature(_))
        ));
    }

    #[test]
    fn each_feature_has_its_own_bit_and_slot() {
        let bits: Vec<i64> = Feature::ALL.iter().map(|f| f.bit()).collect();
        assert_eq!(bits, [1, 2, 4]);
        let slots: Vec<&str> = Feature::ALL.iter().map(|f| f.slot()).collect();
        assert_eq!(slots, ["text", "ocr", "image"]);
    }
}
