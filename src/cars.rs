//! CarOrdinal -> car name lookup, from a bundled community dataset (see the
//! header of assets/car-ordinals.tsv for source and retrieval date). Ordinals
//! missing from the dataset (new Festival Playlist cars) fall back to the number.

use std::collections::HashMap;
use std::sync::OnceLock;

static MAP: OnceLock<HashMap<i32, &'static str>> = OnceLock::new();

pub fn car_name(ordinal: i32) -> Option<&'static str> {
    MAP.get_or_init(|| {
        include_str!("../assets/car-ordinals.tsv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .filter_map(|l| {
                let (ord, name) = l.split_once('\t')?;
                Some((ord.parse().ok()?, name))
            })
            .collect()
    })
    .get(&ordinal)
    .copied()
}

/// "1986 Audi Sport Quattro S2 (#1478)" when known, "car ordinal 1478" when not.
pub fn car_label(ordinal: i32) -> String {
    match car_name(ordinal) {
        Some(name) => format!("{name} (#{ordinal})"),
        None => format!("car ordinal {ordinal}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_ordinals_resolve() {
        assert_eq!(car_name(1478), Some("1986 Audi Sport Quattro S2"));
        assert_eq!(car_name(348), Some("2005 Ford GT"));
        assert_eq!(car_name(-1), None);
        assert_eq!(car_label(2937), "2017 Ford Fiesta GRC (#2937)");
        assert_eq!(car_label(999999), "car ordinal 999999");
    }
}
