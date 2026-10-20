//! Audio tap on mpv's real output (issue #71).
//!
//! libmpv has no PCM callback, so the tap is an mpv audio filter (`@vivi-viz`) appended
//! last to the `af` chain, i.e. after the EQ/compressor/reverb filters. Inside that
//! lavfi graph the audio passes through unchanged while a side branch downmixes to
//! mono, splits it into [`BANDS`] log-spaced band-pass filters (plus the full band),
//! measures the RMS of every band per ~20 ms frame (`astats`) and prints the values
//! with the frame's timestamp to a localhost TCP socket (`ametadata=mode=print`).
//! A reader thread here parses them; the visualizer picks the frame matching mpv's
//! `time-pos` (the filter runs ahead of the speakers by the audio buffer).
//!
//! Works for anything mpv plays (local files, network streams, video files' audio).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const LABEL: &str = "@vivi-viz";
pub const BANDS: usize = 48;
const F_LO: f32 = 30.0;
const F_HI: f32 = 14_000.0;
/// Frames kept (≈ 4 s at ~47 frames/s).
const KEEP: usize = 200;

/// Centre frequency of band `i`.
pub fn band_freq(i: usize) -> f32 {
    band_freq_at(i as f32)
}

/// Frequency at fractional band position `x` (log scale between band centres).
pub fn band_freq_at(x: f32) -> f32 {
    F_LO * (F_HI / F_LO).powf(x / (BANDS - 1) as f32)
}

/// One measured frame: timestamp (s) and linear RMS (0..1) of the full band and each band.
#[derive(Clone, Debug)]
pub struct Frame {
    pub t: f64,
    pub full: f32,
    pub bands: [f32; BANDS],
}

pub struct Tap {
    port: Option<u16>,
    frames: Arc<Mutex<VecDeque<Frame>>>,
}

impl Tap {
    pub fn new() -> Self {
        let frames = Arc::new(Mutex::new(VecDeque::new()));
        let conn = Arc::new(AtomicU64::new(0));
        let listener = TcpListener::bind("127.0.0.1:0").ok();
        let port = listener
            .as_ref()
            .and_then(|l| l.local_addr().ok())
            .map(|a| a.port());
        if let Some(l) = listener {
            let (f, c) = (frames.clone(), conn.clone());
            let _ = std::thread::Builder::new()
                .name("vivi-viz-tap".into())
                .spawn(move || {
                    for s in l.incoming().flatten() {
                        let id = c.fetch_add(1, Ordering::SeqCst) + 1;
                        if let Ok(mut q) = f.lock() {
                            q.clear();
                        }
                        let (f, c) = (f.clone(), c.clone());
                        let _ = std::thread::Builder::new()
                            .name("vivi-viz-conn".into())
                            .spawn(move || read_conn(s, id, &f, &c));
                    }
                });
        }
        Self { port, frames }
    }

    /// The mpv `af` filter (without label) feeding this tap, or None without a socket.
    pub fn filter(&self) -> Option<String> {
        self.port.map(graph)
    }

    /// Drop buffered frames (filter removed / media changed).
    pub fn clear(&self) {
        if let Ok(mut q) = self.frames.lock() {
            q.clear();
        }
    }

    /// The frame playing at time `t` (seconds, mpv `time-pos`). When timestamps do not
    /// line up at all (some streams), the newest frame is used instead.
    pub fn read(&self, t: f64) -> Option<Frame> {
        let mut q = self.frames.lock().ok()?;
        while q.len() > 2 && q[1].t <= t - 1.0 {
            q.pop_front();
        }
        let (first, last) = (q.front()?.t, q.back()?.t);
        if t < first - 2.0 || t > last + 2.0 {
            return q.back().cloned();
        }
        let f = q.iter().rev().find(|f| f.t <= t + 0.01)?;
        (t - f.t < 0.5).then(|| f.clone())
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.frames.lock().unwrap().len()
    }
}

/// lavfi graph: pass-through output plus the measuring side branch.
fn graph(port: u16) -> String {
    let n = BANDS + 1;
    let outs: String = (0..BANDS).map(|i| format!("[b{i}]")).collect();
    let ins: String = (0..BANDS).map(|i| format!("[c{i}]")).collect();
    let bp: String = (0..BANDS)
        .map(|i| format!("[b{i}]bandpass=f={:.1}:t=q:w=4[c{i}];", band_freq(i)))
        .collect();
    format!(
        "lavfi=[asetnsamples=n=1024:p=0,asplit[vo][vt];\
         [vt]aformat=channel_layouts=mono,asplit={n}[full]{outs};{bp}\
         [full]{ins}amerge=inputs={n},\
         astats=metadata=1:reset=1:measure_perchannel=RMS_level:measure_overall=none,\
         ametadata=mode=print:direct=1:file=tcp\\\\://127.0.0.1\\\\:{port},anullsink;\
         [vo]anull]"
    )
}

fn read_conn(s: TcpStream, id: u64, frames: &Mutex<VecDeque<Frame>>, conn: &AtomicU64) {
    let mut cur: Option<Frame> = None;
    let flush = |f: Option<Frame>| {
        if let (Some(f), Ok(mut q)) = (f, frames.lock()) {
            // An older graph still draining must not mix into the new one.
            if conn.load(Ordering::SeqCst) == id {
                if q.back().is_some_and(|b| f.t + 0.5 < b.t) {
                    q.clear(); // jumped back (seek)
                }
                q.push_back(f);
                while q.len() > KEEP {
                    q.pop_front();
                }
            }
        }
    };
    for line in BufReader::new(s).lines() {
        let Ok(line) = line else { break };
        if line.starts_with("frame:") {
            flush(cur.take());
            let t = line
                .split_whitespace()
                .find_map(|w| w.strip_prefix("pts_time:"))
                .and_then(|v| v.parse::<f64>().ok());
            cur = t.map(|t| Frame {
                t,
                full: 0.0,
                bands: [0.0; BANDS],
            });
        } else if let Some(rest) = line.strip_prefix("lavfi.astats.") {
            let (Some(f), Some((ch, v))) = (cur.as_mut(), rest.split_once(".RMS_level=")) else {
                continue;
            };
            let (Ok(ch), Ok(db)) = (ch.parse::<usize>(), v.trim().parse::<f32>()) else {
                continue; // "-inf" (silence) stays 0
            };
            let lin = 10f32.powf(db / 20.0);
            match ch {
                1 => f.full = lin,
                c if c >= 2 && c - 2 < BANDS => f.bands[c - 2] = lin,
                _ => {}
            }
        }
    }
    flush(cur);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn graph_shape() {
        let g = graph(4242);
        assert!(g.starts_with("lavfi=[") && g.ends_with(']'));
        assert_eq!(g.matches("bandpass=").count(), BANDS);
        assert!(g.contains(r"tcp\\://127.0.0.1\\:4242"));
        assert!((band_freq(0) - F_LO).abs() < 0.01 && (band_freq(BANDS - 1) - F_HI).abs() < 1.0);
    }

    #[test]
    fn parses_frames() {
        let tap = Tap::new();
        let port = tap.port.unwrap();
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        use std::io::Write;
        let mut txt = String::new();
        for k in 0..3 {
            txt += &format!("frame:{k}    pts:{k}       pts_time:{}\n", k as f64 * 0.02);
            txt += "lavfi.astats.1.RMS_level=-20.0\n";
            txt += "lavfi.astats.2.RMS_level=-inf\n";
            txt += "lavfi.astats.24.RMS_level=-6.0\n";
        }
        s.write_all(txt.as_bytes()).unwrap();
        drop(s);
        for _ in 0..200 {
            if tap.len() == 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let f = tap.read(0.025).unwrap();
        assert!((f.t - 0.02).abs() < 1e-9);
        assert!((f.full - 0.1).abs() < 1e-3);
        assert_eq!(f.bands[0], 0.0);
        assert!((f.bands[22] - 0.501).abs() < 1e-2);
        // Far-off timestamps: newest frame.
        assert!((tap.read(100.0).unwrap().t - 0.04).abs() < 1e-9);
    }

    /// Plays `url` through libmpv with an optional filter before the tap and returns a
    /// tap frame from mid-playback.
    fn play(url: &str, pre: Option<&str>) -> Frame {
        let mpv = libmpv2::Mpv::new().unwrap();
        mpv.set_property("ao", "null").unwrap();
        mpv.set_property("vo", "null").unwrap();
        let q = |a: &str| format!("\"{}\"", a.replace('\\', "\\\\"));
        if let Some(f) = pre {
            mpv.command("af", &["add", &q(f)]).unwrap();
        }
        let tap = Tap::new();
        let f = format!("{LABEL}:{}", tap.filter().unwrap());
        mpv.command("af", &["add", &q(&f)]).unwrap();
        mpv.command("loadfile", &[&q(url)]).unwrap();
        for _ in 0..500 {
            std::thread::sleep(Duration::from_millis(10));
            let t: f64 = mpv.get_property("time-pos").unwrap_or(0.0);
            if t > 0.5 {
                if let Some(f) = tap.read(t) {
                    let chain = mpv.get_property::<String>("af").unwrap();
                    assert!(super::super::tap_is_last(&chain), "{chain}");
                    return f;
                }
            }
        }
        panic!("no tap frames for {url}");
    }

    fn peak(f: &Frame) -> f32 {
        let i = (0..BANDS)
            .max_by(|&a, &b| f.bands[a].total_cmp(&f.bands[b]))
            .unwrap();
        band_freq(i)
    }

    /// End to end through libmpv: the tone reaches the tap, filters before it count.
    #[test]
    fn mpv_feeds_tap() {
        let wav = std::env::temp_dir().join("vivi-viz-tap-440.wav");
        write_wav(&wav, 440.0, 3.0);
        let url = wav.to_string_lossy().into_owned();
        let f = play(&url, None);
        assert!((peak(&f) / 440.0).log2().abs() < 0.2, "peak {}", peak(&f));
        // The note survives the band measurement (A, not the nearest band centre).
        assert_eq!(super::super::tests::pitch_of(&f), 9);
        assert!(f.full > 0.2, "{}", f.full);
        // An "EQ" in front (-20 dB) is reflected.
        let q = play(&url, Some("@vivi-eq:lavfi=[volume=0.1]"));
        assert!(q.full < f.full * 0.2, "{} vs {}", q.full, f.full);
        let _ = std::fs::remove_file(wav);
    }

    /// A network stream (HTTP, served locally) works the same way.
    #[test]
    fn mpv_feeds_tap_from_stream() {
        let wav = std::env::temp_dir().join("vivi-viz-tap-1k.wav");
        write_wav(&wav, 1000.0, 3.0);
        let body = std::fs::read(&wav).unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for mut s in l.incoming().flatten() {
                let mut req = [0u8; 2048];
                let _ = s.read(&mut req);
                let _ = s.write_all(b"HTTP/1.0 200 OK\r\nContent-Type: audio/wav\r\n\r\n");
                let _ = s.write_all(&body);
            }
        });
        let f = play(&format!("http://127.0.0.1:{port}/live"), None);
        assert!((peak(&f) / 1000.0).log2().abs() < 0.2, "peak {}", peak(&f));
        let _ = std::fs::remove_file(wav);
    }

    fn write_wav(path: &std::path::Path, hz: f32, secs: f32) {
        let rate = 44_100u32;
        let n = (rate as f32 * secs) as u32;
        let mut d = Vec::new();
        let data_len = n * 2;
        d.extend(b"RIFF");
        d.extend((36 + data_len).to_le_bytes());
        d.extend(b"WAVEfmt ");
        d.extend(16u32.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(rate.to_le_bytes());
        d.extend((rate * 2).to_le_bytes());
        d.extend(2u16.to_le_bytes());
        d.extend(16u16.to_le_bytes());
        d.extend(b"data");
        d.extend(data_len.to_le_bytes());
        for i in 0..n {
            let v = ((std::f32::consts::TAU * hz * i as f32 / rate as f32).sin() * 16000.0) as i16;
            d.extend(v.to_le_bytes());
        }
        std::fs::write(path, d).unwrap();
    }
}
