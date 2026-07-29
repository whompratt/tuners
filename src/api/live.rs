//! Live telemetry, recorder, and data-quality views.

use super::*;

/// Latest telemetry frame, cut down to what the Drive screen shows.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FrameView {
    pub race_on: bool,
    /// Car ordinal from the packet (0 in menus, where everything but the timestamp
    /// is zeroed while race is off). Lets onboarding show "car detected".
    pub car: i32,
    pub car_name: Option<String>,
    pub speed_mps: f32,
    pub rpm: f32,
    pub max_rpm: f32,
    pub gear: u8,
    pub lap_number: u16,
    pub current_lap_s: f32,
    pub last_lap_s: f32,
    pub best_lap_s: f32,
    pub fuel: f32,
    pub tire_temp_f: [f32; 4],
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RecorderView {
    /// "recording" | "waiting" | "external" (port busy or disabled: view-only).
    pub mode: String,
    pub file: Option<String>,
    pub packets: u32,
    /// Milliseconds since ANY datagram hit the socket; menu packets count,
    /// though they are never recorded. The onboarding wiring check: fresh
    /// here + no frame = hooked up, just not driving yet. None in external
    /// mode (another capture owns the socket) or before the first packet.
    pub udp_age_ms: Option<u32>,
    /// Car seen in raw packets (free roam counts; nothing need be recorded).
    pub udp_car: Option<i32>,
    pub udp_car_name: Option<String>,
}

/// The `live-state` event payload (was the SSE `state` event).
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LiveStateView {
    pub file: Option<String>,
    pub age_ms: Option<u32>,
    pub frame: Option<FrameView>,
    pub recorder: RecorderView,
}

pub fn live_state_view(
    s: &crate::telemetry::live::LiveState,
    r: &crate::telemetry::record::RecorderStatus,
) -> LiveStateView {
    let frame = s.latest.as_ref().map(|tf| {
        let f = &tf.frame;
        let t = f.tire_temp;
        FrameView {
            race_on: f.is_race_on,
            car: f.car_ordinal,
            car_name: crate::cars::car_name(f.car_ordinal).map(str::to_string),
            speed_mps: f.speed,
            rpm: f.current_engine_rpm,
            max_rpm: f.engine_max_rpm,
            gear: f.gear,
            lap_number: f.lap_number,
            current_lap_s: f.current_lap,
            last_lap_s: f.last_lap,
            best_lap_s: f.best_lap,
            fuel: f.fuel,
            tire_temp_f: [t.fl, t.fr, t.rl, t.rr],
        }
    });
    LiveStateView {
        file: file_name(s.file.as_deref()),
        age_ms: s
            .last_data
            .map(|t| t.elapsed().as_millis().min(u32::MAX as u128) as u32),
        frame,
        recorder: recorder_view(r),
    }
}

pub fn recorder_view(r: &crate::telemetry::record::RecorderStatus) -> RecorderView {
    let mode = match &r.mode {
        crate::telemetry::record::RecorderMode::External(_) => "external",
        crate::telemetry::record::RecorderMode::Waiting => "waiting",
        crate::telemetry::record::RecorderMode::Recording => "recording",
    };
    RecorderView {
        mode: mode.to_string(),
        file: file_name(r.file.as_deref()),
        packets: r.packets.min(u32::MAX as u64) as u32,
        udp_age_ms: r
            .last_udp
            .map(|t| t.elapsed().as_millis().min(u32::MAX as u128) as u32),
        udp_car: r.udp_car,
        udp_car_name: r
            .udp_car
            .and_then(crate::cars::car_name)
            .map(str::to_string),
    }
}

fn file_name(p: Option<&Path>) -> Option<String> {
    p.and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
}

/// The `quality` event payload; None until a comparable lap exists.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QualityView {
    pub laps: u32,
    pub standing_only: bool,
    pub best_lap_s: f32,
    pub spread_pct: f32,
    pub shared_km: f32,
    pub confidence_pct: f32,
    pub band: String,
}

pub fn quality_view(q: Option<&crate::telemetry::live::Quality>) -> Option<QualityView> {
    q.map(|q| QualityView {
        laps: q.laps as u32,
        standing_only: q.standing_only,
        best_lap_s: q.best_lap_s,
        spread_pct: q.spread_frac * 100.0,
        shared_km: q.shared_km,
        confidence_pct: q.confidence * 100.0,
        band: q.band.as_str().to_string(),
    })
}
