//! The table of query terms. A corpus token that no query uses is a miss, so nothing is
//! ever stored for it.

/// The last key byte of a token over 12 bytes. No token byte has this value, so a long token
/// can never equal a shorter token that shares its first bytes.
const LONG: u32 = 0xFF << 24;

/// Bytes of the key's two parts that a token of each length keeps, and the mark to add.
const KEEP: [(u64, u32, u32); 14] = {
    let mut keep = [(0, 0, 0); 14];
    let mut len: usize = 1;
    while len <= 12 {
        let (low, high) = (if len < 8 { len } else { 8 }, len.saturating_sub(8));
        keep[len].0 = if low == 8 { !0 } else { (1 << (low * 8)) - 1 };
        keep[len].1 = if high == 4 { !0 } else { (1 << (high * 8)) - 1 };
        len += 1;
    }
    keep[13] = (!0, !0 >> 8, LONG);
    keep
};

/// The first bytes of a token, lowercase and zero-padded. No token holds a zero byte, so for
/// tokens of up to 12 bytes two equal keys mean two equal tokens, with no further comparison.
#[derive(Clone, Copy, PartialEq)]
pub struct Key {
    low: u64,
    high: u32,
}

impl Key {
    /// The key of the `len`-byte token starting at `bytes[start]`, which must hold only ASCII
    /// letters and digits. Setting bit 0x20 lowercases a letter and leaves a digit unchanged.
    #[inline]
    pub fn at(bytes: &[u8], start: usize, len: usize) -> Key {
        const LOWERCASE: u64 = 0x2020_2020_2020_2020;
        let mut window = [0u8; 12];
        match bytes.get(start..start + 12) {
            Some(full) => window.copy_from_slice(full),
            // Too close to the end of the file for a 12-byte read.
            None => window[..bytes.len() - start].copy_from_slice(&bytes[start..]),
        }
        let low = u64::from_le_bytes(window[..8].try_into().expect("8 bytes")) | LOWERCASE;
        let high = u32::from_le_bytes(window[8..].try_into().expect("4 bytes")) | LOWERCASE as u32;
        let (keep_low, keep_high, mark) = KEEP[len.min(13)];
        Key {
            low: low & keep_low,
            high: (high & keep_high) | mark,
        }
    }

    #[inline]
    fn is_long(self) -> bool {
        self.high & LONG == LONG
    }

    #[inline]
    fn hash(self) -> u64 {
        const A: u64 = 0x9E37_79B9_7F4A_7C15;
        const B: u64 = 0xC2B2_AE3D_27D4_EB4F;
        self.low.wrapping_mul(A) ^ (self.high as u64).wrapping_mul(B)
    }
}

/// A term id with two more bits, kept together because the scan needs them for every token.
#[derive(Clone, Copy)]
pub struct Found(u32);

/// Set in the first entry of a line whose terms continued into the next line.
const OVERFLOW: u32 = 2;

impl Found {
    #[inline]
    pub fn id(self) -> u32 {
        self.0 >> 2
    }

    /// Whether some phrase is anchored on this term.
    #[inline]
    pub fn is_anchor(self) -> bool {
        self.0 & 1 == 1
    }
}

/// One cache line of the table, so that a lookup reads one line and no separate key array.
#[derive(Clone, Copy, Default)]
#[repr(C, align(64))]
struct Line {
    low: [u64; 4],
    high: [u32; 4],
    found: [u32; 4],
}

impl Line {
    /// Which of the four entries hold `key`, as bits.
    #[inline]
    fn matches(&self, key: Key) -> u32 {
        let mut hits = 0;
        for i in 0..4 {
            hits |= ((self.low[i] == key.low) as u32 & (self.high[i] == key.high) as u32) << i;
        }
        hits
    }
}

/// Bits in the filter's index. 2^14 words of 64 bits is 128 KB, small enough to stay in L2.
const FILTER_BITS: u32 = 14;

/// Hash table from term bytes to a dense id: four entries to a line, with terms inserted
/// busiest first, so busy terms sit in their home line.
///
/// A corpus token that no query uses is a miss. Most of those are rare, so they would land on
/// lines that are not in cache. The filter turns almost all of them away first, and they go to
/// an empty line that is always in cache, so a miss costs no branch and no slow memory read.
/// Every hit is an exact comparison, because the key holds the token up to 12 bytes. Longer
/// tokens are compared in full.
#[derive(Default)]
pub struct TermTable<'a> {
    lines: Vec<Line>,
    filter: Vec<u64>,
    terms: Vec<&'a [u8]>,
    shift: u32,
}

impl<'a> TermTable<'a> {
    /// `max_terms` bounds the number of distinct terms.
    pub fn with_capacity(max_terms: usize) -> Self {
        let mut table = Self::default();
        table.reset_lines(max_terms);
        table
    }

    /// Empties the table, sized for about 2.3 terms a line. One more line, the last, stays
    /// empty, and no hash reaches it.
    fn reset_lines(&mut self, terms: usize) {
        let size = (terms / 2).max(2).next_power_of_two();
        self.lines = vec![Line::default(); size + 1];
        self.filter = vec![0; 1 << FILTER_BITS];
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
    fn filter_position(hash: u64) -> (usize, u64) {
        let word = (hash >> (64 - FILTER_BITS)) as usize;
        (word, 1 << (hash >> 30 & 63) | 1 << (hash >> 36 & 63))
    }

    /// The line and entry holding this token, if it is a term, walking every line it can be in.
    fn find(&self, key: Key, token: &[u8]) -> Option<(usize, usize)> {
        let home = (key.hash() >> self.shift) as usize;
        let mask = self.lines.len() - 2;
        let mut at = home;
        loop {
            let line = &self.lines[at];
            let mut hits = line.matches(key);
            while hits != 0 {
                let entry = hits.trailing_zeros() as usize;
                hits &= hits - 1;
                let id = Found(line.found[entry]).id();
                if !key.is_long() || self.terms[id as usize].eq_ignore_ascii_case(token) {
                    return Some((at, entry));
                }
            }
            if line.found[0] & OVERFLOW == 0 {
                return None;
            }
            at = (at + 1) & mask;
        }
    }

    /// Adds a term known not to be in the table yet.
    fn insert(&mut self, key: Key, term: &'a [u8]) {
        let id = self.terms.len() as u32;
        self.terms.push(term);
        let hash = key.hash();
        let (word, bits) = Self::filter_position(hash);
        self.filter[word] |= bits;
        let mask = self.lines.len() - 2;
        let mut at = (hash >> self.shift) as usize;
        loop {
            let line = &mut self.lines[at];
            if let Some(entry) = line.low.iter().position(|&low| low == 0) {
                line.low[entry] = key.low;
                line.high[entry] = key.high;
                line.found[entry] |= id << 2;
                return;
            }
            line.found[0] |= OVERFLOW;
            at = (at + 1) & mask;
        }
    }

    /// The id of `term`, adding it if this is its first appearance.
    pub fn intern(&mut self, term: &'a [u8]) -> u32 {
        let key = Key::at(term, 0, term.len());
        match self.find(key, term) {
            Some((line, entry)) => Found(self.lines[line].found[entry]).id(),
            None => {
                self.insert(key, term);
                self.terms.len() as u32 - 1
            }
        }
    }

    /// Marks a term as the anchor of at least one phrase.
    pub fn mark_anchor(&mut self, id: u32) {
        let term = self.terms[id as usize];
        let (line, entry) = self
            .find(Key::at(term, 0, term.len()), term)
            .expect("a term is in the table");
        self.lines[line].found[entry] |= 1;
    }

    /// Where to look for a token: its home line, or the empty line if the filter rejects it.
    /// Either way the line starts loading now, because a document's tokens are known in
    /// advance, so the slow reads of rare terms can overlap.
    #[inline]
    pub fn route(&self, key: Key) -> u32 {
        let hash = key.hash();
        let (word, bits) = Self::filter_position(hash);
        let empty = self.lines.len() as u32 - 1;
        let home = (hash >> self.shift) as u32;
        let _ = (word, bits, empty);
        #[cfg(target_arch = "x86_64")]
        // SAFETY: `home` is the empty line or a hash shifted down to the table's size, and
        // prefetching has no effect other than on the cache.
        unsafe {
            use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
            _mm_prefetch::<_MM_HINT_T0>(self.lines.as_ptr().add(home as usize).cast());
        }
        home
    }

    /// What a corpus token is, from the line `route` chose. A miss comes back as `miss_id()`,
    /// not anchored.
    #[inline]
    pub fn lookup(&self, home: u32, key: Key, token: &[u8]) -> Found {
        let line = &self.lines[home as usize];
        let hits = line.matches(key);
        let hit = line.found[hits.trailing_zeros() as usize & 3] & !OVERFLOW;
        // Without branches: whether a token is a hit is close to a coin flip.
        let miss = self.miss_id() << 2;
        let answer = [miss, hit][(hits != 0) as usize];
        // Long tokens need their full bytes, and a full line may have sent a term onwards.
        if key.is_long() | ((hits == 0) & (line.found[0] & OVERFLOW != 0)) {
            return match self.find(key, token) {
                Some((at, entry)) => Found(self.lines[at].found[entry] & !OVERFLOW),
                None => Found(miss),
            };
        }
        Found(answer)
    }

    /// Replaces every id with `new_id[id]` and refits the table to the real number of terms.
    /// Terms go back in id order, so the busiest ones get their home lines.
    pub fn renumber(&mut self, new_id: &[u32]) {
        let old_terms = std::mem::take(&mut self.terms);
        let mut order: Vec<usize> = (0..old_terms.len()).collect();
        order.sort_by_key(|&old| new_id[old]);
        self.reset_lines(old_terms.len());
        for old in order {
            let term = old_terms[old];
            self.insert(Key::at(term, 0, term.len()), term);
        }
    }
}
