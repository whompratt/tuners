//! Telemetry I/O: the Data Out wire format, recorded .ftel files, and the
//! ways frames get in and out (UDP capture/recorder, live tailing, replay).

pub mod capture;
pub mod live;
pub mod packet;
pub mod record;
pub mod replay;
pub mod simulate;
pub mod stint;
