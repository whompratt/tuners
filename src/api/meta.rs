//! Static/meta lookups: the car dataset, effect-field registry, and
//! effect-map status.

use super::*;

/// One entry of the effect-field registry: stable key, display label, unit
/// hint ("" = plain number, "frac" = 0..1 shown as %), and the library noise
/// floor. The engine owns this list (`effects::FIELDS`); the frontend must
/// never hand-copy it.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EffectFieldView {
    pub key: String,
    pub label: String,
    pub unit: String,
    pub floor: f32,
}

/// One car of the bundled ordinal->name dataset, for pickers: the user should
/// never have to look an ordinal up themselves.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CarView {
    pub car: i32,
    pub name: String,
}

/// All known cars, name-sorted.
pub fn car_list() -> Vec<CarView> {
    crate::cars::all()
        .into_iter()
        .map(|(car, name)| CarView {
            car,
            name: name.to_string(),
        })
        .collect()
}

pub fn effect_fields() -> Vec<EffectFieldView> {
    crate::analysis::effects::FIELDS
        .iter()
        .map(|(key, label, unit)| EffectFieldView {
            key: key.to_string(),
            label: label.to_string(),
            unit: unit.to_string(),
            floor: crate::analysis::effects::noise_floor(key),
        })
        .collect()
}

// ------------------------------------------------------------------- sharing

/// Effect-map state for the Settings screen: what the background refresher
/// last produced. None = no map yet (nothing journaled anywhere).
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EffectMapStatus {
    pub samples: u32,
    pub campaigns: u32,
    /// Unix ms of the map file's last write.
    pub updated_ms: f64,
}

pub fn effect_map_status() -> Option<EffectMapStatus> {
    let path = crate::util::data_path("effect-map.tsv");
    let text = std::fs::read_to_string(&path).ok()?;
    let map = crate::advice::effectmap::parse(&text).ok()?;
    let updated_ms = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as f64;
    Some(EffectMapStatus {
        samples: map.samples.len() as u32,
        campaigns: map.floors.len() as u32,
        updated_ms,
    })
}
