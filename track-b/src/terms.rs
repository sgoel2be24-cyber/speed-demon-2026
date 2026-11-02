//! The table of query terms. A corpus token that no query uses is a miss, so nothing is
//! ever stored for it.

/// A free slot while the queries are being read, when the number of terms is still unknown.
const EMPTY: u32 = u32::MAX;

/// The last key byte of a token over 16 bytes. No token byte has this value, so a long token
/// can never equal a shorter token that shares its first bytes.
const LONG: u64 = 0xFF << 56;

/// Bytes of the key's two words that a token of each length keeps, and the mark to add.
const KEEP: [[u64; 3]; 18] = {
    let mut keep = [[0; 3]; 18];
    let mut len: usize = 1;
    while len <= 16 {
        let (low, high) = (if len < 8 { len } else { 8 }, len.saturating_sub(8));
        keep[len][0] = if low == 8 { !0 } else { (1 << (low * 8)) - 1 };
        keep[len][1] = if high == 8 { !0 } else { (1 << (high * 8)) - 1 };
        len += 1;
    }
    keep[17] = [!0, !0 >> 8, LONG];
    keep
};

/// The first bytes of a token, lowercase and zero-padded. No token holds a zero byte, so for
/// tokens of up to 16 bytes two equal keys mean two equal tokens, with no further comparison.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Key([u64; 2]);

impl Key {
    /// The key of the `len`-byte token starting at `bytes[start]`, which must hold only ASCII
    /// letters and digits. Setting bit 0x20 lowercases a letter and leaves a digit unchanged.
    #[inline]
    pub fn at(bytes: &[u8], start: usize, len: usize) -> Key {
        const LOWERCASE: u64 = 0x2020_2020_2020_2020;
        let [low, high] = match bytes.get(start..start + 16) {
            Some(full) => [&full[..8], &full[8..]]
                .map(|half| u64::from_le_bytes(half.try_into().expect("8 bytes"))),
            None => near_end(bytes, start),
        };
        let [low, high] = [low | LOWERCASE, high | LOWERCASE];
        let [keep_low, keep_high, mark] = KEEP[len.min(17)];
        Key([low & keep_low, (high & keep_high) | mark])
    }

    #[inline]
    fn is_long(self) -> bool {
        self.0[1] & LONG == LONG
    }

    #[inline]
    fn hash(self) -> u64 {
        const A: u64 = 0x9E37_79B9_7F4A_7C15;
        const B: u64 = 0xC2B2_AE3D_27D4_EB4F;
        self.0[0].wrapping_mul(A) ^ self.0[1].wrapping_mul(B)
    }
}

/// The bytes at `start`, zero-padded, for a token too close to the end for a 16-byte read.
#[cold]
fn near_end(bytes: &[u8], start: usize) -> [u64; 2] {
    let mut window = [0u8; 16];
    window[..bytes.len() - start].copy_from_slice(&bytes[start..]);
    [&window[..8], &window[8..]].map(|half| u64::from_le_bytes(half.try_into().expect("8 bytes")))
}

/// A term id with one more bit, kept together because the scan needs both for every token.
#[derive(Clone, Copy)]
pub struct Found(u32);

impl Found {
    #[inline]
    pub fn id(self) -> u32 {
        self.0 >> 1
    }

    /// Whether some phrase is anchored on this term.
    #[inline]
    pub fn is_anchor(self) -> bool {
        self.0 & 1 == 1
    }
}

/// Open-addressing hash table from term bytes to a dense id.
///
/// A slot only says where to look. Keys live in a separate array indexed by id, so with ids
/// in order of use the busiest keys share cache lines. Every hit is an exact comparison: the
/// corpus holds about 600,000 distinct tokens, so accepting a mere hash match would change
/// an answer sooner or later.
///
/// A slot holds a `Found`. Tokens that are not query terms get the id `miss_id()`, one past
/// the last term, so the scan treats them like any term and needs no branch for them.
#[derive(Default)]
pub struct TermTable<'a> {
    slots: Vec<u32>,
    keys: Vec<Key>,
    terms: Vec<&'a [u8]>,
    shift: u32,
    /// What a free slot holds. Once all terms are known this is a `Found` with `miss_id()`,
    /// and `keys` has one more entry for that id, which equals no token's key.
    empty: u32,
}

impl<'a> TermTable<'a> {
    /// `max_terms` bounds the number of distinct terms. This table only has to survive parsing,
    /// and `renumber` refits it, so it stays small enough not to cost cache misses of its own.
    pub fn with_capacity(max_terms: usize) -> Self {
        let mut table = Self {
            empty: EMPTY,
            ..Self::default()
        };
        table.reset_slots(max_terms + 1);
        table
    }

    /// Empties the slots, making room for at least `size`.
    fn reset_slots(&mut self, size: usize) {
        let size = size.max(2).next_power_of_two();
        self.slots = vec![self.empty; size];
        self.shift = 64 - size.trailing_zeros();
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    /// The id every miss gets. No query holds it, so a phrase can never match across a miss.
    pub fn miss_id(&self) -> u32 {
        self.terms.len() as u32
    }

    pub fn term(&self, id: u32) -> &'a [u8] {
        self.terms[id as usize]
    }

    #[inline]
    fn home(&self, key: Key) -> usize {
        (key.hash() >> self.shift) as usize
    }

    /// The slot holding this token, or the free slot where it belongs, starting from `home`.
    fn find(&self, home: usize, key: Key, token: &[u8]) -> usize {
        let mask = self.slots.len() - 1;
        let mut i = home;
        loop {
            let slot = self.slots[i];
            if slot == self.empty {
                return i;
            }
            let id = Found(slot).id() as usize;
            if self.keys[id] == key
                && (!key.is_long() || self.terms[id].eq_ignore_ascii_case(token))
            {
                return i;
            }
            i = (i + 1) & mask;
        }
    }

    /// The id of `term`, adding it if this is its first appearance.
    pub fn intern(&mut self, term: &'a [u8]) -> u32 {
        let key = Key::at(term, 0, term.len());
        let slot = self.find(self.home(key), key, term);
        if self.slots[slot] == self.empty {
            self.slots[slot] = (self.terms.len() as u32) << 1;
            self.keys.push(key);
            self.terms.push(term);
        }
        Found(self.slots[slot]).id()
    }

    /// Marks a term as the anchor of at least one phrase.
    pub fn mark_anchor(&mut self, id: u32) {
        let term = self.terms[id as usize];
        let key = Key::at(term, 0, term.len());
        let slot = self.find(self.home(key), key, term);
        self.slots[slot] |= 1;
    }

    /// The slot `key` hashes to, after starting to load it into the cache. Rare tokens land
    /// on slots that are not cached, and a document's tokens are known in advance, so those
    /// misses can overlap. Passing the slot to `lookup` saves computing the hash twice.
    #[inline]
    pub fn home_only(&self, key: Key) -> usize {
        self.home(key)
    }

    #[inline]
    pub fn prefetch(&self, key: Key) -> usize {
        let home = self.home(key);
        #[cfg(target_arch = "x86_64")]
        // SAFETY: `home` is inside `slots`, and prefetching only affects the cache.
        unsafe {
            use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
            _mm_prefetch::<_MM_HINT_T0>(self.slots.as_ptr().add(home).cast());
        }
        home
    }

    /// What a corpus token is, from the home slot that `prefetch` gave. A miss comes back as
    /// `miss_id()`, not anchored.
    ///
    /// The slot settles nearly every token with no branch on whether it is a hit, which is
    /// close to a coin flip: a free slot already holds the answer for a miss, and its id leads
    /// to a key that matches nothing.
    #[inline(always)]
    pub fn lookup(&self, home: usize, key: Key, bytes: &[u8], start: usize, end: usize) -> Found {
        let slot = self.slots[home];
        let settled = (self.keys[(slot >> 1) as usize] == key) | (slot == self.empty);
        if !settled | key.is_long() {
            return self.probe(home, key, &bytes[start..end]);
        }
        Found(slot)
    }

    /// The rest of a lookup, for a token whose home slot holds another term, or a long token.
    #[inline(never)]
    fn probe(&self, home: usize, key: Key, token: &[u8]) -> Found {
        Found(self.slots[self.find(home, key, token)])
    }

    /// Replaces every id with `new_id[id]` and refits the slots to the real number of terms.
    /// Terms go back in id order, so the busiest ones get the shortest probes.
    pub fn renumber(&mut self, new_id: &[u32]) {
        let old_terms = std::mem::take(&mut self.terms);
        let mut order: Vec<usize> = (0..old_terms.len()).collect();
        order.sort_by_key(|&old| new_id[old]);
        self.keys.clear();
        self.empty = (old_terms.len() as u32) << 1;
        self.reset_slots(old_terms.len() * 3);
        for old in order {
            self.intern(old_terms[old]);
        }
        self.keys.push(Key::default());
    }
}
