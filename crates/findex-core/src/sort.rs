use std::cmp::Ordering;

use crate::Entry;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Type,
    Size,
}

impl SortKey {
    pub fn from_index(i: i32) -> SortKey {
        match i {
            1 => SortKey::Modified,
            2 => SortKey::Type,
            3 => SortKey::Size,
            _ => SortKey::Name,
        }
    }
    pub fn index(self) -> i32 {
        self as i32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

/// Explorer-style natural order: case-insensitive, digit runs compared by value ("file2" < "file10").
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let da = take_digits(&mut a);
                let db = take_digits(&mut b);
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut s = String::new();
    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
        s.push(c);
        it.next();
    }
    s
}

fn cmp_entries(a: &Entry, b: &Entry, sort: Sort) -> Ordering {
    // Folders stay on top in both directions, as in Explorer.
    let folders = b.is_dir().cmp(&a.is_dir());
    if folders != Ordering::Equal {
        return folders;
    }
    let ord = match sort.key {
        SortKey::Name => Ordering::Equal,
        SortKey::Modified => a.mtime.cmp(&b.mtime),
        SortKey::Type => a.kind.label().cmp(b.kind.label()),
        SortKey::Size => a.size.cmp(&b.size),
    }
    .then_with(|| natural_cmp(&a.name, &b.name));
    if sort.descending { ord.reverse() } else { ord }
}

/// Returns entry indices in display order. Stable: ties keep listing order.
pub fn sort_order(entries: &[Entry], sort: Sort) -> Vec<u32> {
    let mut order: Vec<u32> = (0..entries.len() as u32).collect();
    order.sort_by(|&x, &y| cmp_entries(&entries[x as usize], &entries[y as usize], sort));
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "File2", "file1", "a", "file02b"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a", "file1", "File2", "file02b", "file10"]);
    }

    #[test]
    fn folders_first_even_descending() {
        let e = vec![
            Entry::new("b.txt".into(), false, 5, 0, 0),
            Entry::new("zdir".into(), true, 0, 0, 0),
            Entry::new("a.txt".into(), false, 9, 0, 0),
        ];
        let o = sort_order(&e, Sort { key: SortKey::Name, descending: true });
        assert_eq!(o, [1, 0, 2]);
        let o = sort_order(&e, Sort { key: SortKey::Size, descending: false });
        assert_eq!(o, [1, 0, 2]);
    }
}
