/// SpecId and their activation block
/// Information was obtained from: https://github.com/ethereum/execution-specs
#[repr(u8)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, enumn::N)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(non_camel_case_types)]
pub enum SpecId {
    LATEST = 0
}

impl SpecId {
    pub fn try_from_u8(spec_id: u8) -> Option<Self> {
        Self::n(spec_id)
    }
}

pub use SpecId::*;

impl From<&str> for SpecId {
    fn from(name: &str) -> Self {
        match name {
            _ => SpecId::LATEST,
        }
    }
}

impl SpecId {
    #[inline]
    pub const fn enabled(our: SpecId, other: SpecId) -> bool {
        our as u8 >= other as u8
    }
}

pub trait Spec: Sized {
    #[inline(always)]
    fn enabled(spec_id: SpecId) -> bool {
        Self::SPEC_ID as u8 >= spec_id as u8
    }
    const SPEC_ID: SpecId;
}

macro_rules! spec {
    ($spec_id:tt,$spec_name:tt) => {
        pub struct $spec_name;

        impl Spec for $spec_name {
            //specification id
            const SPEC_ID: SpecId = $spec_id;
        }
    };
}

spec!(LATEST, LatestSpec);
