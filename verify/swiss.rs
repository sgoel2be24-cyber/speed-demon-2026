// Experimental standard-library SwissTable backend. Exact keys and full long-token
// comparisons are retained; no answer is accepted from a hash alone.
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};

#[derive(Default)]
struct FastHasher(u64);

impl Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0 ^ (self.0 >> 32)
    }
    fn write_u128(&mut self, value: u128) {
        self.0 = (value as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ ((value >> 64) as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    }
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = self.0.wrapping_mul(0x100_0000_01b3) ^ byte as u64;
        }
    }
}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u128(self.0[0] as u128 | ((self.0[1] as u128) << 64));
    }
}

#[derive(Default)]
pub struct TermTable<'a> {
    map: HashMap<Key, u32, BuildHasherDefault<FastHasher>>,
    terms: Vec<&'a [u8]>,
    next: Vec<u32>,
    anchors: Vec<bool>,
}

impl<'a> TermTable<'a> {
    pub fn with_capacity(max_terms: usize) -> Self {
        Self {
            map: HashMap::with_capacity_and_hasher(max_terms, BuildHasherDefault::default()),
            ..Self::default()
        }
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }
    pub fn term(&self, id: u32) -> &'a [u8] {
        self.terms[id as usize]
    }

    fn find(&self, key: Key, token: &[u8]) -> Option<u32> {
        let mut id = *self.map.get(&key)? >> 1;
        if !key.is_long() {
            return Some(id);
        }
        while id != EMPTY {
            if self.terms[id as usize].eq_ignore_ascii_case(token) {
                return Some(id);
            }
            id = self.next[id as usize];
        }
        None
    }

    pub fn intern(&mut self, term: &'a [u8]) -> u32 {
        let key = Key::at(term, 0, term.len());
        if let Some(id) = self.find(key, term) {
            return id;
        }
        let id = self.terms.len() as u32;
        let next = self
            .map
            .insert(key, id << 1)
            .map_or(EMPTY, |packed| packed >> 1);
        self.terms.push(term);
        self.next.push(next);
        self.anchors.push(false);
        id
    }

    pub fn mark_anchor(&mut self, id: u32) {
        self.anchors[id as usize] = true;
        let term = self.terms[id as usize];
        let key = Key::at(term, 0, term.len());
        let packed = self.map.get_mut(&key).unwrap();
        if *packed >> 1 == id {
            *packed |= 1
        }
    }

    #[inline]
    pub fn prefetch(&self, _key: Key) -> usize {
        0
    }

    #[inline(always)]
    pub fn lookup(&self, _home: usize, key: Key, bytes: &[u8], start: usize, end: usize) -> Found {
        if key.is_long() {
            return self
                .find(key, &bytes[start..end])
                .map_or(Found((self.len() as u32) << 1), |id| {
                    Found((id << 1) | u32::from(self.anchors[id as usize]))
                });
        }
        Found(
            self.map
                .get(&key)
                .copied()
                .unwrap_or((self.len() as u32) << 1),
        )
    }

    pub fn renumber(&mut self, order: &[u32]) {
        let old = std::mem::take(&mut self.terms);
        *self = Self::with_capacity(old.len());
        for &id in order {
            self.intern(old[id as usize]);
        }
    }
}
