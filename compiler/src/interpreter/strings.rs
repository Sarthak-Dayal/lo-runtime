use std::cmp::Ordering;

use super::abort::AbortKind;
use super::heap::{Heap, StrId};

/// `(a + b)` — byte concatenation.
pub fn concat(heap: &mut Heap, a: StrId, b: StrId) -> Result<StrId, AbortKind> {
    let sa = heap.str_value(a);
    let sb = heap.str_value(b);
    let mut out = String::with_capacity(sa.len() + sb.len());
    out.push_str(&sa);
    out.push_str(&sb);
    heap.alloc_str(&out)
}

/// `(s * n)` — `n` copies. Negative `n` aborts (120); zero yields the empty string.
pub fn repeat(heap: &mut Heap, s: StrId, n: i32) -> Result<StrId, AbortKind> {
    if n < 0 {
        return Err(AbortKind::RepeatNegative(n));
    }
    if n == 0 {
        return Ok(heap.empty());
    }
    let out = heap.str_value(s).repeat(n as usize);
    heap.alloc_str(&out)
}

/// `(~s)` — reverse by Unicode codepoint (not by byte).
pub fn reverse(heap: &mut Heap, s: StrId) -> Result<StrId, AbortKind> {
    let out: String = heap.str_value(s).chars().rev().collect();
    heap.alloc_str(&out)
}

/// `(a < b)` / `(a > b)` / `(a = b)` — lexicographic over UTF-8 bytes.
pub fn compare(heap: &Heap, a: StrId, b: StrId) -> Ordering {
    heap.str_value(a)
        .as_bytes()
        .cmp(heap.str_value(b).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::super::heap::DEFAULT_HEAP_LIMIT;
    use super::*;

    fn s(heap: &mut Heap, text: &str) -> StrId {
        heap.alloc_str(text).unwrap()
    }

    #[test]
    fn concat_joins_bytes() {
        let mut heap = Heap::new(DEFAULT_HEAP_LIMIT);
        let a = s(&mut heap, "ab");
        let b = s(&mut heap, "cd");
        let r = concat(&mut heap, a, b).unwrap();
        assert_eq!(&*heap.str_value(r), "abcd");
    }

    #[test]
    fn repeat_counts_and_edge_cases() {
        let mut heap = Heap::new(DEFAULT_HEAP_LIMIT);
        let x = s(&mut heap, "ab");
        let r3 = repeat(&mut heap, x, 3).unwrap();
        assert_eq!(&*heap.str_value(r3), "ababab");
        let empty = heap.empty();
        assert_eq!(repeat(&mut heap, x, 0).unwrap(), empty);
        assert_eq!(
            repeat(&mut heap, x, -1).unwrap_err(),
            AbortKind::RepeatNegative(-1)
        );
    }

    #[test]
    fn reverse_is_by_codepoint() {
        let mut heap = Heap::new(DEFAULT_HEAP_LIMIT);
        let abc = s(&mut heap, "abc");
        let r = reverse(&mut heap, abc).unwrap();
        assert_eq!(&*heap.str_value(r), "cba");
        // Multibyte: "aé" reversed is "éa" (é is 2 UTF-8 bytes); a byte reverse
        // would corrupt it.
        let m = s(&mut heap, "aé");
        let rm = reverse(&mut heap, m).unwrap();
        assert_eq!(&*heap.str_value(rm), "éa");
    }

    #[test]
    fn compare_is_codepoint_order() {
        let mut heap = Heap::new(DEFAULT_HEAP_LIMIT);
        // 'Z' (U+005A) < 'a' (U+0061).
        let z = s(&mut heap, "Z");
        let a = s(&mut heap, "a");
        assert_eq!(compare(&heap, z, a), Ordering::Less);
        let abc = s(&mut heap, "abc");
        let abc2 = s(&mut heap, "abc");
        assert_eq!(compare(&heap, abc, abc2), Ordering::Equal);
        let abd = s(&mut heap, "abd");
        assert_eq!(compare(&heap, abc, abd), Ordering::Less);
    }
}
