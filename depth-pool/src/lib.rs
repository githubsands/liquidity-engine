use rkyv::{
    Archive, Deserialize, Serialize, access, access_unchecked, api::high::to_bytes_with_alloc,
    deserialize, rancor::Error, ser::allocator::Arena, util::AlignedVec,
};

#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone, PartialEq)]
#[rkyv(compare(PartialEq), derive(Debug, PartialEq))]
pub struct DepthUpdate {
    pub k: u8,
    pub p: f64,
    pub q: f64,
    pub l: u8,
    pub s: bool,
}

impl Default for DepthUpdate {
    fn default() -> Self {
        DepthUpdate {
            k: 0,
            p: 0.0,
            q: 0.0,
            l: 0,
            s: false,
        }
    }
}

impl DepthUpdate {
    pub fn to_bytes(&self) -> Result<AlignedVec, Error> {
        let mut arena = Arena::new();
        self.serialize_with(&mut arena)
    }
    pub fn serialize_with(&self, arena: &mut Arena) -> Result<AlignedVec, Error> {
        to_bytes_with_alloc::<_, Error>(self, arena.acquire())
    }
}

pub fn access_depth_update(bytes: &[u8]) -> Result<&ArchivedDepthUpdate, Error> {
    access::<ArchivedDepthUpdate, Error>(bytes)
}
pub unsafe fn access_depth_update_unchecked(bytes: &[u8]) -> &ArchivedDepthUpdate {
    unsafe { access_unchecked::<ArchivedDepthUpdate>(bytes) }
}

pub fn deserialize_depth_update(archived: &ArchivedDepthUpdate) -> Result<DepthUpdate, Error> {
    deserialize::<DepthUpdate, Error>(archived)
}

impl ArchivedDepthUpdate {
    pub fn to_owned(&self) -> Result<DepthUpdate, Error> {
        deserialize::<DepthUpdate, Error>(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DepthUpdate {
        DepthUpdate {
            k: 1,
            p: 27123.45,
            q: 0.001234,
            l: 3,
            s: true,
        }
    }

    #[test]
    fn zero_copy_access_matches_original() {
        let update = sample();
        let bytes = update.to_bytes().expect("serialize");

        let archived = access_depth_update(&bytes).expect("access");

        assert_eq!(archived, &update);
        assert_eq!(archived.k, 1);
        assert_eq!(archived.l, 3);
        assert_eq!(archived.s, true);
        assert_eq!(archived.p.to_native(), 27123.45);
        assert_eq!(archived.q.to_native(), 0.001234);
    }

    #[test]
    fn round_trip_deserialize() {
        let update = sample();
        let bytes = update.to_bytes().expect("serialize");
        let archived = access_depth_update(&bytes).expect("access");
        let recovered = deserialize_depth_update(archived).expect("deserialize");
        assert_eq!(recovered, update);
        assert_eq!(archived.to_owned().expect("to_owned"), update);
    }

    #[test]
    fn unchecked_access_matches_checked() {
        let update = sample();
        let bytes = update.to_bytes().expect("serialize");
        let checked = access_depth_update(&bytes).expect("access");
        let unchecked = unsafe { access_depth_update_unchecked(&bytes) };
        assert_eq!(checked, unchecked);
    }

    #[test]
    fn arena_reuse_produces_identical_bytes() {
        let update = sample();
        let mut arena = Arena::new();
        let a = update.serialize_with(&mut arena).expect("serialize a");
        let b = update.serialize_with(&mut arena).expect("serialize b");
        assert_eq!(a.as_slice(), b.as_slice());

        let archived = access_depth_update(&b).expect("access");
        assert_eq!(archived, &update);
    }

    #[test]
    fn rejects_garbage_bytes() {
        let garbage = [0xFFu8; 4];
        assert!(access_depth_update(&garbage).is_err());
    }
}
