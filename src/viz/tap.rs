//! Parallel PCM tap: a second, lightweight decode of the playing file (symphonia) on a
//! worker thread, kept a little ahead of mpv's `time-pos`. Only a few seconds of mono
//! samples are buffered, so memory stays tiny regardless of track length.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

/// Seconds decoded ahead of the play position / kept behind it.
const AHEAD: f64 = 1.5;
const BEHIND: f64 = 1.0;

#[derive(Default)]
struct Ring {
    rate: u32,
    /// Timestamp (seconds) of `samples[0]`.
    start: f64,
    samples: VecDeque<f32>,
}

impl Ring {
    fn end(&self) -> f64 {
        self.start + self.samples.len() as f64 / self.rate.max(1) as f64
    }
}

pub struct Tap {
    ring: Arc<Mutex<Ring>>,
    target: Arc<AtomicU64>,
    ok: Arc<AtomicBool>,
    tx: Sender<Option<String>>,
}

impl Tap {
    pub fn new() -> Self {
        let ring = Arc::new(Mutex::new(Ring::default()));
        let target = Arc::new(AtomicU64::new(0f64.to_bits()));
        let ok = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let (r, t, o) = (ring.clone(), target.clone(), ok.clone());
        std::thread::Builder::new()
            .name("vivi-viz-tap".into())
            .spawn(move || worker(rx, r, t, o))
            .expect("spawn viz tap");
        Self {
            ring,
            target,
            ok,
            tx,
        }
    }

    /// Start decoding `path` (None stops and frees the decoder).
    pub fn open(&self, path: Option<String>) {
        self.ok.store(false, Ordering::Relaxed);
        let _ = self.tx.send(path);
    }

    /// True once the file was opened by the decoder (false = unsupported/failed).
    pub fn ok(&self) -> bool {
        self.ok.load(Ordering::Relaxed)
    }

    /// Copy `out.len()` mono samples ending at time `t` into `out`.
    /// Returns the sample rate, or None when that span is not buffered (yet).
    pub fn read(&self, t: f64, out: &mut [f32]) -> Option<u32> {
        self.target.store(t.to_bits(), Ordering::Relaxed);
        let ring = self.ring.lock().ok()?;
        if ring.rate == 0 {
            return None;
        }
        let end_idx = ((t - ring.start) * ring.rate as f64).round() as i64;
        let start_idx = end_idx - out.len() as i64;
        if start_idx < 0 || end_idx > ring.samples.len() as i64 {
            return None;
        }
        for (o, s) in out.iter_mut().zip(ring.samples.range(start_idx as usize..)) {
            *o = *s;
        }
        Some(ring.rate)
    }
}

struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    rate: u32,
    buf: Option<SampleBuffer<f32>>,
}

fn open(path: &str) -> Option<Source> {
    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .ok()?;
    let format = probed.format;
    let track = format.tracks().iter().find(|t| {
        t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some()
    })?;
    let rate = track.codec_params.sample_rate?;
    let decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .ok()?;
    Some(Source {
        track: track.id,
        format,
        decoder,
        rate,
        buf: None,
    })
}

impl Source {
    fn seek(&mut self, t: f64) -> Option<f64> {
        let r = self
            .format
            .seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time: Time::from(t.max(0.0)),
                    track_id: Some(self.track),
                },
            )
            .ok()?;
        self.decoder.reset();
        Some(r.actual_ts as f64 / self.rate as f64)
    }

    /// Decode the next packet as mono; returns (timestamp, samples). None = end/error.
    fn next(&mut self, out: &mut Vec<f32>) -> Option<f64> {
        loop {
            let packet = self.format.next_packet().ok()?;
            if packet.track_id() != self.track {
                continue;
            }
            let ts = packet.ts() as f64 / self.rate as f64;
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(_) => return None,
            };
            let spec = *decoded.spec();
            let ch = spec.channels.count().max(1);
            let need = decoded.capacity() as u64;
            if self
                .buf
                .as_ref()
                .is_none_or(|b| b.capacity() < need as usize * ch)
            {
                self.buf = Some(SampleBuffer::new(need, spec));
            }
            let buf = self.buf.as_mut()?;
            buf.copy_interleaved_ref(decoded);
            out.clear();
            out.extend(
                buf.samples()
                    .chunks_exact(ch)
                    .map(|f| f.iter().sum::<f32>() / ch as f32),
            );
            return Some(ts);
        }
    }
}

fn worker(
    rx: Receiver<Option<String>>,
    ring: Arc<Mutex<Ring>>,
    target: Arc<AtomicU64>,
    ok: Arc<AtomicBool>,
) {
    let mut src: Option<Source> = None;
    let mut chunk = Vec::new();
    let mut eof = false;
    loop {
        // Idle: block for commands. Otherwise poll quickly between packets.
        let wait = if src.is_none() {
            Duration::from_secs(3600)
        } else {
            Duration::ZERO
        };
        match rx.recv_timeout(wait) {
            Ok(path) => {
                src = path.as_deref().and_then(open);
                eof = false;
                let mut r = ring.lock().unwrap();
                r.samples.clear();
                r.start = 0.0;
                r.rate = src.as_ref().map_or(0, |s| s.rate);
                ok.store(src.is_some(), Ordering::Relaxed);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let Some(s) = src.as_mut() else { continue };
        let t = f64::from_bits(target.load(Ordering::Relaxed));
        let (start, end) = {
            let r = ring.lock().unwrap();
            (r.start, r.end())
        };
        // Seek when the play position left the buffered window (user seek, new file).
        if t < start - 0.05 || t > end + 0.5 {
            eof = false;
            let at = s.seek((t - 0.25).max(0.0)).unwrap_or(t);
            let mut r = ring.lock().unwrap();
            r.samples.clear();
            r.start = at;
            continue;
        }
        if eof || end > t + AHEAD {
            std::thread::sleep(Duration::from_millis(15));
            continue;
        }
        match s.next(&mut chunk) {
            Some(ts) => {
                let mut r = ring.lock().unwrap();
                let rate = r.rate as f64;
                if r.samples.is_empty() {
                    r.start = ts;
                } else if (ts - r.end()).abs() > 0.2 {
                    // Discontinuity (coarse seek landed elsewhere): restart the window.
                    r.samples.clear();
                    r.start = ts;
                }
                r.samples.extend(chunk.iter().copied());
                let excess = ((t - BEHIND - r.start) * rate).floor();
                if excess > 0.0 {
                    let n = (excess as usize).min(r.samples.len());
                    r.samples.drain(..n);
                    r.start += n as f64 / rate;
                }
            }
            None => eof = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3 s of 16-bit stereo 44.1 kHz WAV: 220 Hz tone.
    fn wav(path: &std::path::Path) {
        let rate = 44_100u32;
        let n = rate * 3;
        let mut d = Vec::new();
        let data_len = n * 4;
        d.extend(b"RIFF");
        d.extend((36 + data_len).to_le_bytes());
        d.extend(b"WAVEfmt ");
        d.extend(16u32.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(2u16.to_le_bytes());
        d.extend(rate.to_le_bytes());
        d.extend((rate * 4).to_le_bytes());
        d.extend(4u16.to_le_bytes());
        d.extend(16u16.to_le_bytes());
        d.extend(b"data");
        d.extend(data_len.to_le_bytes());
        for i in 0..n {
            let v =
                ((std::f32::consts::TAU * 220.0 * i as f32 / rate as f32).sin() * 16000.0) as i16;
            d.extend(v.to_le_bytes());
            d.extend(v.to_le_bytes());
        }
        std::fs::write(path, d).unwrap();
    }

    #[test]
    fn follows_position_and_seeks() {
        let p = std::env::temp_dir().join("vivi-viz-tap-test.wav");
        wav(&p);
        let tap = Tap::new();
        tap.open(Some(p.to_string_lossy().into_owned()));
        let mut out = vec![0.0; 2048];
        let read = |t: f64, out: &mut [f32]| {
            for _ in 0..200 {
                if let Some(r) = tap.read(t, out) {
                    return Some(r);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            None
        };
        assert_eq!(read(0.5, &mut out), Some(44_100));
        assert!(out.iter().any(|s| s.abs() > 0.3));
        // Jump forward (seek) and back.
        assert!(read(2.5, &mut out).is_some());
        assert!(read(0.3, &mut out).is_some());
        assert!(tap.ok());
        let _ = std::fs::remove_file(p);
    }
}
