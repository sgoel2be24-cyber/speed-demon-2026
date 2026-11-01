//! The table of query terms. A corpus token that no query uses is a miss, so nothing is
//! ever stored for it.

/// The id of a token that is not a query term. No query holds this id, so a phrase
/// comparison can never match across a miss.
pub const NONE: u32 = u32::MAX;

/// A free slot. It equals `NONE`, so a lookup that ends on a free slot reports a miss.
const EMPTY: u32 = NONE;

/// Open-addressing hash table from term bytes to a dense id.
///
/// A slot only says where to look. Every hit compares the full term, because the corpus
/// holds about 600,000 distinct tokens and a fingerprint collision would change an answer.
pub struct TermTable<'a> {
    slots: Vec<u32>,
    terms: Vec<&'a [u8]>,
    mask: usize,
}

fn hash(key: &[u8]) -> usize {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let h = key
        .iter()
        .fold(FNV_OFFSET, |h, &b| (h ^ b as u64).wrapping_mul(FNV_PRIME));
    (h ^ (h >> 32)) as usize
}

impl<'a> TermTable<'a> {
    /// `max_terms` bounds the number of distinct terms. The table stays under half full.
    pub fn with_capacity(max_terms: usize) -> Self {
        let size = (max_terms * 2).next_power_of_two();
        TermTable {
            slots: vec![EMPTY; size],
            terms: Vec::new(),
            mask: size - 1,
        }
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn term(&self, id: u32) -> &'a [u8] {
        self.terms[id as usize]
    }

    /// The slot holding `key`, or the empty slot where it belongs.
    fn find(&self, key: &[u8]) -> usize {
        let mut i = hash(key) & self.mask;
        loop {
            let id = self.slots[i];
            if id == EMPTY || self.terms[id as usize] == key {
                return i;
            }
            i = (i + 1) & self.mask;
        }
    }

    /// The id of `term`, adding it if this is its first appearance.
    pub fn intern(&mut self, term: &'a [u8]) -> u32 {
        let slot = self.find(term);
        if self.slots[slot] == EMPTY {
            self.slots[slot] = self.terms.len() as u32;
            self.terms.push(term);
        }
        self.slots[slot]
    }

    /// The id of a lowercase corpus token, or `NONE` when no query uses it.
    #[inline]
    pub fn lookup(&self, token: &[u8]) -> u32 {
        self.slots[self.find(token)]
    }

    /// Replaces every id with `new_id[id]`.
    pub fn renumber(&mut self, new_id: &[u32]) {
        let mut terms: Vec<&[u8]> = vec![&[]; self.terms.len()];
        for (old, &new) in new_id.iter().enumerate() {
            terms[new as usize] = self.terms[old];
        }
        self.terms = terms;
        for slot in self.slots.iter_mut().filter(|slot| **slot != EMPTY) {
            *slot = new_id[*slot as usize];
        }
    }
}
