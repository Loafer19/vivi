//! Audio waveform for the seekbar: ~400 peak levels decoded once with symphonia
//! (no ffmpeg) on a background thread, cached in `~/.cache/vivi/peaks/<hash>.txt`.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::AppWindow;

pub const N: usize = 400;

fn cache_file(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok().hash(&mut h);
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(
        base.join("vivi/peaks")
            .join(format!("{:016x}.txt", h.finish())),
    )
}

/// Peak levels (0..1, normalised so the loudest bucket is 1) of the first audio track.
pub fn compute(path: &Path) -> Result<Vec<f32>, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(e) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(e);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("no audio track")?;
    let tid = track.id;
    let frames = track.codec_params.n_frames.ok_or("unknown length")?.max(1);
    let mut dec = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;
    let mut peaks = vec![0f32; N];
    let mut pos: u64 = 0;
    let mut buf: Option<SampleBuffer<f32>> = None;
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != tid {
            continue;
        }
        let d = match dec.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(_) => break,
        };
        let spec = *d.spec();
        let ch = spec.channels.count().max(1);
        let need = d.capacity();
        if buf.as_ref().is_none_or(|b| b.capacity() < need * ch) {
            buf = Some(SampleBuffer::new(need as u64, spec));
        }
        let b = buf.as_mut().ok_or("buffer")?;
        b.copy_interleaved_ref(d);
        for f in b.samples().chunks(ch) {
            let i = ((pos * N as u64) / frames).min(N as u64 - 1) as usize;
            let v = f.iter().fold(0f32, |m, s| m.max(s.abs()));
            if v > peaks[i] {
                peaks[i] = v;
            }
            pos += 1;
        }
    }
    Ok(normalise(peaks))
}

fn normalise(mut p: Vec<f32>) -> Vec<f32> {
    let max = p.iter().cloned().fold(0f32, f32::max);
    if max > 0.0 {
        p.iter_mut().for_each(|v| *v = (*v / max).sqrt().max(0.04));
    }
    p
}

fn load_or_compute(path: &Path) -> Option<Vec<f32>> {
    let cache = cache_file(path);
    if let Some(s) = cache.as_ref().and_then(|c| std::fs::read_to_string(c).ok()) {
        let v: Vec<f32> = s
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        if v.len() == N {
            return Some(v);
        }
    }
    let v = compute(path).ok()?;
    if let Some(c) = cache {
        let _ = std::fs::create_dir_all(c.parent()?);
        let txt: Vec<String> = v.iter().map(|x| format!("{x:.3}")).collect();
        let _ = std::fs::write(c, txt.join(" "));
    }
    Some(v)
}

/// Clear the waveform, then (if `on` and `path` is a local file) fill it in the background.
pub fn start(app: &AppWindow, path: &str, on: bool) {
    app.set_wave_peaks(ModelRc::default());
    app.set_wave_path(path.into());
    let p = PathBuf::from(path);
    if !on || !p.is_file() {
        return;
    }
    let weak = app.as_weak();
    let want = path.to_string();
    std::thread::spawn(move || {
        let Some(v) = load_or_compute(&p) else { return };
        let _ = weak.upgrade_in_event_loop(move |a| {
            if a.get_wave_path() == want.as_str() {
                a.set_wave_peaks(ModelRc::new(VecModel::from(v)));
            }
        });
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn normalise_scales_to_one() {
        let v = super::normalise(vec![0.0, 0.25, 0.5]);
        assert_eq!(v[2], 1.0);
        assert!(v[0] > 0.0 && v[1] < 1.0);
    }
}
