//! FH6 "Data Out" packet decoding/encoding.
//!
//! Layout is transcribed from docs/telemetry.md; field order and widths must match
//! it exactly. Any quirk discovered against real captures gets recorded there first.

/// Total datagram size the game sends.
pub const PACKET_LEN: usize = 324;
/// Bytes covered by documented fields; the final byte is undocumented.
pub const DECODED_LEN: usize = 323;

/// Per-wheel values, always ordered front-left, front-right, rear-left, rear-right.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Corners<T> {
    pub fl: T,
    pub fr: T,
    pub rl: T,
    pub rr: T,
}

impl<T: Copy> Corners<T> {
    pub fn to_array(self) -> [T; 4] {
        [self.fl, self.fr, self.rl, self.rr]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TelemetryFrame {
    pub is_race_on: bool,
    /// In-game milliseconds; can overflow to 0.
    pub timestamp_ms: u32,
    pub engine_max_rpm: f32,
    pub engine_idle_rpm: f32,
    pub current_engine_rpm: f32,
    /// Car local space; X = right, Y = up, Z = forward.
    pub acceleration: [f32; 3],
    pub velocity: [f32; 3],
    /// rad/s; X = pitch, Y = yaw, Z = roll.
    pub angular_velocity: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    /// 0.0 = max stretch, 1.0 = max compression.
    pub norm_suspension_travel: Corners<f32>,
    /// 0 = full grip, |x| > 1 = grip loss (same for slip angle / combined slip).
    pub tire_slip_ratio: Corners<f32>,
    /// rad/s.
    pub wheel_rotation_speed: Corners<f32>,
    pub wheel_on_rumble_strip: Corners<bool>,
    pub wheel_in_puddle: Corners<bool>,
    pub surface_rumble: Corners<f32>,
    pub tire_slip_angle: Corners<f32>,
    pub tire_combined_slip: Corners<f32>,
    /// Meters.
    pub suspension_travel_meters: Corners<f32>,
    pub car_ordinal: i32,
    /// 0 (D) – 7 (X).
    pub car_class: i32,
    /// 100–999.
    pub car_performance_index: i32,
    /// 0 = FWD, 1 = RWD, 2 = AWD.
    pub drivetrain_type: i32,
    pub num_cylinders: i32,
    pub car_group: u32,
    pub smashable_vel_diff: f32,
    pub smashable_mass: f32,
    /// World space, meters.
    pub position: [f32; 3],
    /// m/s.
    pub speed: f32,
    /// Watts.
    pub power: f32,
    /// Newton-meters.
    pub torque: f32,
    /// Units unverified, assumed °F (see telemetry.md).
    pub tire_temp: Corners<f32>,
    /// PSI above atmospheric.
    pub boost: f32,
    /// 0.0–1.0.
    pub fuel: f32,
    /// Meters.
    pub distance_traveled: f32,
    /// Seconds; 0.0 if not applicable.
    pub best_lap: f32,
    pub last_lap: f32,
    pub current_lap: f32,
    pub current_race_time: f32,
    pub lap_number: u16,
    pub race_position: u8,
    /// Inputs 0–255.
    pub accel: u8,
    pub brake: u8,
    pub clutch: u8,
    pub handbrake: u8,
    pub gear: u8,
    /// -127 = full left, 127 = full right.
    pub steer: i8,
    pub normalized_driving_line: i8,
    pub normalized_ai_brake_difference: i8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodeError {
    TooShort { len: usize },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::TooShort { len } => {
                write!(
                    f,
                    "packet too short: {len} bytes, need at least {DECODED_LEN}"
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let bytes = self.buf[self.pos..self.pos + N].try_into().unwrap();
        self.pos += N;
        bytes
    }
    fn f32(&mut self) -> f32 {
        f32::from_le_bytes(self.take())
    }
    fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.take())
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take())
    }
    fn u8(&mut self) -> u8 {
        self.take::<1>()[0]
    }
    fn i8(&mut self) -> i8 {
        self.take::<1>()[0] as i8
    }
    fn f32x3(&mut self) -> [f32; 3] {
        [self.f32(), self.f32(), self.f32()]
    }
    fn corners_f32(&mut self) -> Corners<f32> {
        Corners {
            fl: self.f32(),
            fr: self.f32(),
            rl: self.f32(),
            rr: self.f32(),
        }
    }
    fn corners_bool_i32(&mut self) -> Corners<bool> {
        Corners {
            fl: self.i32() != 0,
            fr: self.i32() != 0,
            rl: self.i32() != 0,
            rr: self.i32() != 0,
        }
    }
}

pub fn decode(buf: &[u8]) -> Result<TelemetryFrame, DecodeError> {
    if buf.len() < DECODED_LEN {
        return Err(DecodeError::TooShort { len: buf.len() });
    }
    let mut c = Cursor { buf, pos: 0 };
    let frame = TelemetryFrame {
        is_race_on: c.i32() != 0,
        timestamp_ms: c.u32(),
        engine_max_rpm: c.f32(),
        engine_idle_rpm: c.f32(),
        current_engine_rpm: c.f32(),
        acceleration: c.f32x3(),
        velocity: c.f32x3(),
        angular_velocity: c.f32x3(),
        yaw: c.f32(),
        pitch: c.f32(),
        roll: c.f32(),
        norm_suspension_travel: c.corners_f32(),
        tire_slip_ratio: c.corners_f32(),
        wheel_rotation_speed: c.corners_f32(),
        wheel_on_rumble_strip: c.corners_bool_i32(),
        wheel_in_puddle: c.corners_bool_i32(),
        surface_rumble: c.corners_f32(),
        tire_slip_angle: c.corners_f32(),
        tire_combined_slip: c.corners_f32(),
        suspension_travel_meters: c.corners_f32(),
        car_ordinal: c.i32(),
        car_class: c.i32(),
        car_performance_index: c.i32(),
        drivetrain_type: c.i32(),
        num_cylinders: c.i32(),
        car_group: c.u32(),
        smashable_vel_diff: c.f32(),
        smashable_mass: c.f32(),
        position: c.f32x3(),
        speed: c.f32(),
        power: c.f32(),
        torque: c.f32(),
        tire_temp: c.corners_f32(),
        boost: c.f32(),
        fuel: c.f32(),
        distance_traveled: c.f32(),
        best_lap: c.f32(),
        last_lap: c.f32(),
        current_lap: c.f32(),
        current_race_time: c.f32(),
        lap_number: c.u16(),
        race_position: c.u8(),
        accel: c.u8(),
        brake: c.u8(),
        clutch: c.u8(),
        handbrake: c.u8(),
        gear: c.u8(),
        steer: c.i8(),
        normalized_driving_line: c.i8(),
        normalized_ai_brake_difference: c.i8(),
    };
    debug_assert_eq!(c.pos, DECODED_LEN);
    Ok(frame)
}

struct Enc {
    buf: Vec<u8>,
}

impl Enc {
    fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn i8(&mut self, v: i8) {
        self.buf.push(v as u8);
    }
    fn f32x3(&mut self, v: [f32; 3]) {
        for x in v {
            self.f32(x);
        }
    }
    fn corners_f32(&mut self, c: Corners<f32>) {
        for x in c.to_array() {
            self.f32(x);
        }
    }
    fn corners_bool_i32(&mut self, c: Corners<bool>) {
        for x in c.to_array() {
            self.i32(x as i32);
        }
    }
}

/// Build a wire packet from a frame. Used by `simulate` and tests; the game itself is
/// of course the only real producer.
pub fn encode(f: &TelemetryFrame) -> [u8; PACKET_LEN] {
    let mut e = Enc {
        buf: Vec::with_capacity(PACKET_LEN),
    };
    e.i32(f.is_race_on as i32);
    e.u32(f.timestamp_ms);
    e.f32(f.engine_max_rpm);
    e.f32(f.engine_idle_rpm);
    e.f32(f.current_engine_rpm);
    e.f32x3(f.acceleration);
    e.f32x3(f.velocity);
    e.f32x3(f.angular_velocity);
    e.f32(f.yaw);
    e.f32(f.pitch);
    e.f32(f.roll);
    e.corners_f32(f.norm_suspension_travel);
    e.corners_f32(f.tire_slip_ratio);
    e.corners_f32(f.wheel_rotation_speed);
    e.corners_bool_i32(f.wheel_on_rumble_strip);
    e.corners_bool_i32(f.wheel_in_puddle);
    e.corners_f32(f.surface_rumble);
    e.corners_f32(f.tire_slip_angle);
    e.corners_f32(f.tire_combined_slip);
    e.corners_f32(f.suspension_travel_meters);
    e.i32(f.car_ordinal);
    e.i32(f.car_class);
    e.i32(f.car_performance_index);
    e.i32(f.drivetrain_type);
    e.i32(f.num_cylinders);
    e.u32(f.car_group);
    e.f32(f.smashable_vel_diff);
    e.f32(f.smashable_mass);
    e.f32x3(f.position);
    e.f32(f.speed);
    e.f32(f.power);
    e.f32(f.torque);
    e.corners_f32(f.tire_temp);
    e.f32(f.boost);
    e.f32(f.fuel);
    e.f32(f.distance_traveled);
    e.f32(f.best_lap);
    e.f32(f.last_lap);
    e.f32(f.current_lap);
    e.f32(f.current_race_time);
    e.u16(f.lap_number);
    e.u8(f.race_position);
    e.u8(f.accel);
    e.u8(f.brake);
    e.u8(f.clutch);
    e.u8(f.handbrake);
    e.u8(f.gear);
    e.i8(f.steer);
    e.i8(f.normalized_driving_line);
    e.i8(f.normalized_ai_brake_difference);
    assert_eq!(e.buf.len(), DECODED_LEN);
    e.buf.push(0); // undocumented trailing byte
    e.buf.try_into().unwrap()
}

pub fn class_name(class: i32) -> &'static str {
    match class {
        0 => "D",
        1 => "C",
        2 => "B",
        3 => "A",
        4 => "S1",
        5 => "S2",
        7 => "X",
        _ => "?",
    }
}

pub fn drivetrain_name(dt: i32) -> &'static str {
    match dt {
        0 => "FWD",
        1 => "RWD",
        2 => "AWD",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let frame = crate::telemetry::simulate::synth_frame(2.5);
        let decoded = decode(&encode(&frame)).unwrap();
        assert_eq!(frame, decoded);
    }

    /// Pin fields to the byte offsets documented in docs/telemetry.md.
    #[test]
    fn spot_offsets() {
        let f = TelemetryFrame {
            timestamp_ms: 0xAABB_CCDD,
            car_ordinal: 0x0102_0304,
            car_group: 0x1122_3344,
            speed: 1.0,
            tire_temp: Corners {
                fl: 2.0,
                ..Default::default()
            },
            lap_number: 0xBEEF,
            gear: 7,
            steer: -5,
            ..Default::default()
        };
        let b = encode(&f);
        assert_eq!(b.len(), PACKET_LEN);
        assert_eq!(b[4..8], 0xAABB_CCDDu32.to_le_bytes());
        assert_eq!(b[212..216], 0x0102_0304i32.to_le_bytes());
        assert_eq!(b[232..236], 0x1122_3344u32.to_le_bytes());
        assert_eq!(b[256..260], 1.0f32.to_le_bytes());
        assert_eq!(b[268..272], 2.0f32.to_le_bytes());
        assert_eq!(b[312..314], 0xBEEFu16.to_le_bytes());
        assert_eq!(b[319], 7);
        assert_eq!(b[320] as i8, -5);
    }

    #[test]
    fn rejects_short_packet() {
        assert_eq!(decode(&[0u8; 100]), Err(DecodeError::TooShort { len: 100 }));
    }

    /// A real 324-byte datagram decodes even though only 323 bytes are documented.
    #[test]
    fn accepts_full_and_documented_lengths() {
        let b = encode(&TelemetryFrame::default());
        assert!(decode(&b).is_ok());
        assert!(decode(&b[..DECODED_LEN]).is_ok());
    }
}
