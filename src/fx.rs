//! Audio/video effects from the Figma "Equalizer" and "Filters" frames: the 10-band
//! equalizer with presets, video adjustments, crop, colour effects, compressor,
//! reverb and delay. Each group becomes one labelled mpv filter (`@vivi-eq`, …) so the
//! groups (and other filters such as `@vivi-norm`) can change independently.
//! Settings persist in `~/.config/vivi/fx.toml`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Equalizer band centres (Hz), as in VLC and the Figma frame.
pub const BANDS: [u32; 10] = [60, 170, 310, 600, 1000, 3000, 6000, 12000, 14000, 16000];

/// Built-in presets: preamp (VLC convention, 12 dB = unity) followed by 10 bands in dB.
/// Bass, Vocal and Night come from the former Equalizer plugin (unity preamp; the
/// auto pre-amp keeps their boosts from clipping).
pub const PRESETS: &[(&str, [f32; 11])] = &[
    (
        "Flat",
        [12.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    (
        "Bass",
        [12.0, 6.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    (
        "Classical",
        [12.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -7.2, -7.2, -7.2, -9.6],
    ),
    (
        "Club",
        [6.0, 0.0, 0.0, 8.0, 5.6, 5.6, 5.6, 3.2, 0.0, 0.0, 0.0],
    ),
    (
        "Dance",
        [5.0, 9.6, 7.2, 2.4, 0.0, 0.0, -5.6, -7.2, -7.2, 0.0, 0.0],
    ),
    (
        "Full bass",
        [
            5.0, -8.0, 9.6, 9.6, 5.6, 1.6, -4.0, -8.0, -10.4, -11.2, -11.2,
        ],
    ),
    (
        "Full bass & treble",
        [4.0, 7.2, 5.6, 0.0, -7.2, -4.8, 1.6, 8.0, 11.2, 12.0, 12.0],
    ),
    (
        "Full treble",
        [
            3.0, -9.6, -9.6, -9.6, -4.0, 2.4, 11.2, 16.0, 16.0, 16.0, 16.8,
        ],
    ),
    (
        "Headphones",
        [4.0, 4.8, 11.2, 5.6, -3.2, -2.4, 1.6, 4.8, 9.6, 12.8, 14.4],
    ),
    (
        "Large hall",
        [5.0, 10.4, 10.4, 5.6, 5.6, 0.0, -4.8, -4.8, -4.8, 0.0, 0.0],
    ),
    (
        "Live",
        [7.0, -4.8, 0.0, 4.0, 5.6, 5.6, 5.6, 4.0, 2.4, 2.4, 2.4],
    ),
    (
        "Night",
        [12.0, -5.0, -3.0, -1.0, 1.0, 2.0, 2.0, 1.0, 0.0, -1.0, -1.0],
    ),
    (
        "Party",
        [6.0, 7.2, 7.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 7.2, 7.2],
    ),
    (
        "Pop",
        [6.0, -1.6, 4.8, 7.2, 8.0, 5.6, 0.0, -2.4, -2.4, -1.6, -1.6],
    ),
    (
        "Reggae",
        [8.0, 0.0, 0.0, 0.0, -5.6, 0.0, 6.4, 6.4, 0.0, 0.0, 0.0],
    ),
    (
        "Rock",
        [5.0, 8.0, 4.8, -5.6, -8.0, -3.2, 4.0, 8.8, 11.2, 11.2, 11.2],
    ),
    (
        "Ska",
        [6.0, -2.4, -4.8, -4.0, 0.0, 4.0, 5.6, 8.8, 9.6, 11.2, 9.6],
    ),
    (
        "Soft",
        [5.0, 4.8, 1.6, 0.0, -2.4, 0.0, 4.0, 8.0, 9.6, 11.2, 12.0],
    ),
    (
        "Soft rock",
        [7.0, 4.0, 4.0, 2.4, 0.0, -4.0, -5.6, -3.2, 0.0, 2.4, 8.8],
    ),
    (
        "Techno",
        [5.0, 8.0, 5.6, 0.0, -5.6, -4.8, 0.0, 8.0, 9.6, 9.6, 8.8],
    ),
    (
        "Vocal",
        [12.0, -2.0, -1.0, 1.0, 3.0, 4.0, 3.0, 2.0, 0.0, -1.0, -1.0],
    ),
];

/// Gradient maps offered by the colour panel (ffmpeg `pseudocolor` presets).
pub const GRADIENTS: [&str; 5] = ["turbo", "magma", "viridis", "heat", "cividis"];

/// Effect groups, their mpv filter kind/label and default values.
/// eq: preamp, 10 bands, on · video: hue, brightness, contrast, saturation, gamma,
/// sharpen, deband, grain · crop: top, bottom, left, right, link · color: extract on,
/// threshold on, blend, similarity, negate, posterize, gradient on, cartoon, sepia,
/// gradient index · comp: on, rms/peak, attack, release, threshold, ratio, knee, makeup
/// · reverb: on, size, width, wet, dry, damp · delay: on, time, feedback, crossfeed, dry mix.
pub const GROUPS: &[(&str, &str, &[f32])] = &[
    (
        "eq",
        "af",
        &[12.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    ("video", "vf", &[0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0]),
    ("crop", "vf", &[0.0, 0.0, 0.0, 0.0, 0.0]),
    (
        "color",
        "vf",
        &[0.0, 0.0, 0.1, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    ("comp", "af", &[0.0, 0.2, 25.0, 100.0, -11.0, 4.0, 5.0, 7.0]),
    ("reverb", "af", &[0.0, 8.0, 10.0, 4.0, 3.0, 3.0]),
    ("delay", "af", &[0.0, 20.0, 20.0, 30.0, 80.0]),
];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub values: Vec<f32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Fx {
    pub groups: BTreeMap<String, Vec<f32>>,
    /// Selected equalizer preset (built-in or custom name).
    pub preset: String,
    pub custom: Vec<Preset>,
    pub extract_color: String,
    pub threshold_color: String,
    /// Auto pre-amp / prevent clipping: lower the preamp by the largest band boost.
    pub auto_preamp: bool,
}

impl Default for Fx {
    fn default() -> Self {
        Self {
            groups: GROUPS
                .iter()
                .map(|(g, _, d)| (g.to_string(), d.to_vec()))
                .collect(),
            preset: "Flat".into(),
            custom: Vec::new(),
            extract_color: "#FF0000".into(),
            threshold_color: "#FF0000".into(),
            auto_preamp: true,
        }
    }
}

pub fn path() -> std::path::PathBuf {
    crate::config::path().with_file_name("fx.toml")
}

fn n(v: f32) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// "#RRGGBB" / "RRGGBB" → "0xRRGGBB" (None when invalid).
fn hex(c: &str) -> Option<String> {
    let h = c.trim().trim_start_matches('#');
    (h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("0x{}", h.to_ascii_uppercase()))
}

fn db(v: f32) -> f32 {
    10f32.powf(v / 20.0)
}

impl Fx {
    pub fn load() -> Self {
        let mut fx: Fx = std::fs::read_to_string(path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        fx.normalize();
        fx
    }

    pub fn save(&self) {
        let p = path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = toml::to_string_pretty(self) {
            if let Err(e) = crate::store::write_atomic(&p, &s) {
                eprintln!("vivi: save {}: {e}", p.display());
            }
        }
    }

    /// Fill missing groups/values with defaults (older or hand-edited files).
    fn normalize(&mut self) {
        for (g, _, d) in GROUPS {
            let v = self.groups.entry(g.to_string()).or_default();
            v.truncate(d.len());
            v.extend_from_slice(&d[v.len()..]);
        }
    }

    pub fn get(&self, group: &str) -> Vec<f32> {
        self.groups.get(group).cloned().unwrap_or_default()
    }

    /// Change one value; returns false for an unknown group/index.
    pub fn set(&mut self, group: &str, i: usize, v: f32) -> bool {
        let link = group == "crop" && i < 4 && self.get("crop").get(4) == Some(&1.0);
        let Some(vals) = self.groups.get_mut(group) else {
            return false;
        };
        if i >= vals.len() || !v.is_finite() {
            return false;
        }
        if link {
            vals[..4].iter_mut().for_each(|x| *x = v);
        } else {
            vals[i] = v;
        }
        if group == "eq" && i < 11 {
            // Editing a band: a custom preset follows the change, a built-in one turns "Custom".
            let cur: Vec<f32> = vals[..11].to_vec();
            if let Some(c) = self.custom.iter_mut().find(|c| c.name == self.preset) {
                c.values = cur;
            } else if self.preset != "Custom" {
                self.preset = "Custom".into();
            }
        }
        true
    }

    pub fn reset(&mut self, group: &str) {
        if let Some((_, _, d)) = GROUPS.iter().find(|(g, _, _)| *g == group) {
            let on = self.get(group).get(11).copied();
            self.groups.insert(group.into(), d.to_vec());
            if group == "eq" {
                // Reset bands but keep the equalizer on/off state.
                if let (Some(on), Some(v)) = (on, self.groups.get_mut("eq")) {
                    v[11] = on;
                }
                self.preset = "Flat".into();
            }
        }
    }

    /// Select a built-in or custom preset (turns the equalizer on).
    pub fn select_preset(&mut self, name: &str) -> bool {
        let vals = PRESETS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.to_vec())
            .or_else(|| {
                self.custom
                    .iter()
                    .find(|c| c.name == name)
                    .map(|c| c.values.clone())
            });
        let Some(vals) = vals else { return false };
        let eq = self.groups.entry("eq".into()).or_default();
        for (i, v) in vals.iter().take(11).enumerate() {
            eq[i] = *v;
        }
        eq[11] = 1.0;
        self.preset = name.into();
        true
    }

    /// Built-in then custom preset names, in the order Alt+E cycles them.
    pub fn preset_names(&self) -> Vec<String> {
        PRESETS
            .iter()
            .map(|(n, _)| n.to_string())
            .chain(self.custom.iter().map(|c| c.name.clone()))
            .collect()
    }

    /// Select the preset after the current one (wrapping; "Custom" starts at the first)
    /// and turn the equalizer on. Returns its name.
    pub fn cycle_preset(&mut self) -> String {
        let names = self.preset_names();
        let next = names
            .iter()
            .position(|n| *n == self.preset)
            .map_or(0, |i| (i + 1) % names.len());
        let name = names[next].clone();
        self.select_preset(&name);
        name
    }

    /// Turn the equalizer on or off; returns the new state.
    pub fn toggle(&mut self) -> bool {
        let on = self.get("eq").get(11).copied().unwrap_or(0.0) <= 0.0;
        self.set("eq", 11, if on { 1.0 } else { 0.0 });
        on
    }

    /// Gain (dB) applied before the bands: preamp − 12, and with the auto pre-amp at most
    /// minus the largest positive band boost, so the loudest band stays at or under 0 dB.
    pub fn effective_preamp(&self) -> f32 {
        eq_gain(&self.get("eq"), self.auto_preamp)
    }

    /// Save the current bands as "Custom Preset N" and select it.
    pub fn new_preset(&mut self) -> String {
        let name = (1..)
            .map(|i| format!("Custom Preset {i:02}"))
            .find(|n| self.custom.iter().all(|c| &c.name != n))
            .unwrap_or_default();
        self.custom.push(Preset {
            name: name.clone(),
            values: self.get("eq")[..11].to_vec(),
        });
        self.preset = name.clone();
        name
    }

    pub fn rename_preset(&mut self, old: &str, new: &str) -> bool {
        let new = new.trim();
        if new.is_empty()
            || PRESETS.iter().any(|(n, _)| *n == new)
            || self.custom.iter().any(|c| c.name == new)
        {
            return false;
        }
        let Some(c) = self.custom.iter_mut().find(|c| c.name == old) else {
            return false;
        };
        c.name = new.into();
        if self.preset == old {
            self.preset = new.into();
        }
        true
    }

    pub fn delete_preset(&mut self, name: &str) {
        self.custom.retain(|c| c.name != name);
        if self.preset == name {
            self.preset = "Custom".into();
        }
    }

    /// Presets as shareable JSON: `{"format": "vivi-eq-presets", "version": 1, "presets":
    /// [{"name", "preamp", "bands": [10 × dB]}]}`. Exports the custom presets, or the
    /// current bands (under the selected preset name) when there are none.
    pub fn export_json(&self) -> String {
        let list: Vec<(String, Vec<f32>)> = if self.custom.is_empty() {
            vec![(self.preset.clone(), self.get("eq")[..11].to_vec())]
        } else {
            self.custom
                .iter()
                .map(|c| (c.name.clone(), c.values.clone()))
                .collect()
        };
        let presets: Vec<serde_json::Value> = list
            .into_iter()
            .map(|(name, v)| {
                serde_json::json!({ "name": name, "preamp": v[0], "bands": v[1..11.min(v.len())] })
            })
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "format": "vivi-eq-presets",
            "version": 1,
            "bands_hz": BANDS,
            "presets": presets,
        }))
        .unwrap_or_default()
    }

    /// Add presets from JSON (the export format, a bare list of presets, or one preset).
    /// Names that clash get " (2)", " (3)", …; values are clamped. Returns the names added.
    pub fn import_json(&mut self, text: &str) -> Result<Vec<String>, String> {
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        let list = match &v {
            serde_json::Value::Array(a) => a.clone(),
            serde_json::Value::Object(o) if o.contains_key("presets") => {
                o["presets"].as_array().cloned().unwrap_or_default()
            }
            serde_json::Value::Object(_) => vec![v.clone()],
            _ => Vec::new(),
        };
        let mut added = Vec::new();
        for p in list {
            let bands: Vec<f32> = p["bands"]
                .as_array()
                .map(|b| {
                    b.iter()
                        .filter_map(|x| x.as_f64())
                        .map(|x| x as f32)
                        .collect()
                })
                .unwrap_or_default();
            if bands.len() != 10 {
                return Err("each preset needs 10 band values".into());
            }
            let pre = p["preamp"].as_f64().unwrap_or(12.0) as f32;
            let base = p["name"].as_str().unwrap_or("Imported").trim().to_string();
            let base = if base.is_empty() {
                "Imported".into()
            } else {
                base
            };
            let taken = |n: &str| {
                PRESETS.iter().any(|(b, _)| *b == n) || self.custom.iter().any(|c| c.name == n)
            };
            let name = if taken(&base) {
                (2..)
                    .map(|i| format!("{base} ({i})"))
                    .find(|n| !taken(n))
                    .unwrap_or_default()
            } else {
                base
            };
            let mut values = vec![pre.clamp(0.0, 20.0)];
            values.extend(bands.iter().map(|b| b.clamp(-20.0, 20.0)));
            self.custom.push(Preset {
                name: name.clone(),
                values,
            });
            added.push(name);
        }
        if added.is_empty() {
            return Err("no presets found".into());
        }
        Ok(added)
    }

    /// mpv filter for a group: (kind "af"/"vf", label, filter or None when inactive).
    pub fn filter(&self, group: &str) -> Option<(&'static str, String, Option<String>)> {
        let (_, kind, _) = GROUPS.iter().find(|(g, _, _)| *g == group)?;
        let v = self.get(group);
        let chain: Vec<String> = match group {
            "eq" => eq_chain(&v, self.auto_preamp),
            "video" => video_chain(&v),
            "crop" => crop_chain(&v),
            "color" => self.color_chain(&v),
            "comp" => comp_chain(&v),
            "reverb" => reverb_chain(&v),
            "delay" => delay_chain(&v),
            _ => Vec::new(),
        };
        let f = (!chain.is_empty()).then(|| format!("lavfi=[{}]", chain.join(",")));
        Some((kind, format!("@vivi-{group}"), f))
    }

    fn color_chain(&self, v: &[f32]) -> Vec<String> {
        let mut c = Vec::new();
        let (blend, sim) = (v[2].clamp(0.0, 1.0), v[3].clamp(0.01, 1.0));
        if v[0] > 0.0 {
            if let Some(h) = hex(&self.extract_color) {
                c.push(format!(
                    "colorhold=color={h}:similarity={}:blend={}",
                    n(sim),
                    n(blend)
                ));
            }
        }
        if v[1] > 0.0 {
            if let Some(h) = hex(&self.threshold_color) {
                c.push(format!(
                    "chromakey=color={h}:similarity={}:blend={}",
                    n(sim),
                    n(blend)
                ));
            }
        }
        if v[4] > 0.0 {
            c.push("negate".into());
        }
        if v[5] > 0.0 {
            c.push("lutrgb=r=floor(val/64)*64:g=floor(val/64)*64:b=floor(val/64)*64".into());
        }
        if v[6] > 0.0 {
            let g = GRADIENTS[(v[9].max(0.0) as usize).min(GRADIENTS.len() - 1)];
            c.push(format!("pseudocolor=preset={g}"));
        }
        if v[7] > 0.0 {
            c.push("edgedetect=mode=colormix:high=0.15".into());
        }
        let s = v[8].clamp(0.0, 1.0);
        if s > 0.0 {
            // Linear blend from identity to the classic sepia matrix.
            let m = [
                [0.393, 0.769, 0.189],
                [0.349, 0.686, 0.168],
                [0.272, 0.534, 0.131],
            ];
            let mut o = Vec::new();
            for (r, ch) in ["r", "g", "b"].iter().enumerate() {
                for (k, src) in ["r", "g", "b"].iter().enumerate() {
                    let id = if r == k { 1.0 } else { 0.0 };
                    o.push(format!("{ch}{src}={}", n(id * (1.0 - s) + m[r][k] * s)));
                }
            }
            c.push(format!("colorchannelmixer={}", o.join(":")));
        }
        c
    }
}

fn eq_gain(v: &[f32], auto: bool) -> f32 {
    let gain = v.first().map_or(0.0, |p| p - 12.0);
    if !auto {
        return gain;
    }
    let boost = v.iter().skip(1).take(10).fold(0f32, |m, g| m.max(*g));
    gain.min(-boost)
}

fn eq_chain(v: &[f32], auto: bool) -> Vec<String> {
    if v[11] <= 0.0 {
        return Vec::new();
    }
    let mut c = Vec::new();
    let gain = eq_gain(v, auto);
    if gain.abs() > 0.05 {
        c.push(format!("volume={}dB", n(gain)));
    }
    for (f, g) in BANDS.iter().zip(&v[1..11]) {
        if g.abs() > 0.05 {
            c.push(format!("equalizer=f={f}:t=o:w=1:g={}", n(*g)));
        }
    }
    c
}

fn video_chain(v: &[f32]) -> Vec<String> {
    let mut c = Vec::new();
    if v[0].abs() > 0.01 {
        c.push(format!("hue=h={}", n(v[0])));
    }
    if v[1].abs() > 0.001
        || (v[2] - 1.0).abs() > 0.001
        || (v[3] - 1.0).abs() > 0.001
        || (v[4] - 1.0).abs() > 0.001
    {
        c.push(format!(
            "eq=brightness={}:contrast={}:saturation={}:gamma={}",
            n(v[1]),
            n(v[2]),
            n(v[3]),
            n(v[4].max(0.1))
        ));
    }
    if v[5] > 0.001 {
        c.push(format!("unsharp=5:5:{}:5:5:0", n(v[5].min(5.0))));
    }
    if v[6] > 0.001 {
        let t = n(0.01 + 0.09 * v[6].min(1.0));
        c.push(format!(
            "deband=1thr={t}:2thr={t}:3thr={t}:4thr={t}:range=16"
        ));
    }
    if v[7] >= 1.0 {
        c.push(format!("noise=alls={}:allf=t", v[7].round().min(100.0)));
    }
    c
}

fn crop_chain(v: &[f32]) -> Vec<String> {
    let [t, b, l, r] = [v[0], v[1], v[2], v[3]].map(|x| x.max(0.0).round() as i64);
    if t + b + l + r == 0 {
        return Vec::new();
    }
    vec![format!("crop=w=iw-{}:h=ih-{}:x={l}:y={t}", l + r, t + b)]
}

fn comp_chain(v: &[f32]) -> Vec<String> {
    if v[0] <= 0.0 {
        return Vec::new();
    }
    vec![format!(
        "acompressor=detection={}:attack={}:release={}:threshold={}:ratio={}:knee={}:makeup={}",
        if v[1] >= 0.5 { "rms" } else { "peak" },
        n(v[2].clamp(0.01, 2000.0)),
        n(v[3].clamp(0.01, 9000.0)),
        n(db(v[4]).clamp(0.000_976_563, 1.0)),
        n(v[5].clamp(1.0, 20.0)),
        n(db(v[6]).clamp(1.0, 8.0)),
        n(db(v[7]).clamp(1.0, 64.0)),
    )]
}

/// ffmpeg has no Freeverb; a four-tap `aecho` plus `extrastereo` approximates it.
fn reverb_chain(v: &[f32]) -> Vec<String> {
    if v[0] <= 0.0 {
        return Vec::new();
    }
    let base = 20.0 + v[1].clamp(0.0, 10.0) * 10.0;
    let decay = 0.6 * (1.0 - v[5].clamp(0.0, 10.0) / 10.0 * 0.7);
    let taps = [1.0, 1.37, 1.73, 2.11];
    let delays: Vec<String> = taps.iter().map(|k| n((base * k).round())).collect();
    let decays: Vec<String> = (0..4).map(|i| n(decay * 0.85f32.powi(i))).collect();
    let mut c = vec![format!(
        "aecho=in_gain={}:out_gain={}:delays={}:decays={}",
        n((v[4] / 10.0).clamp(0.0, 1.0)),
        n((v[3] / 10.0).clamp(0.05, 1.0)),
        delays.join("|"),
        decays.join("|")
    )];
    let m = 0.5 + v[2].clamp(0.0, 10.0) / 10.0;
    if (m - 1.0).abs() > 0.01 {
        c.push(format!("extrastereo=m={}", n(m)));
    }
    c
}

fn delay_chain(v: &[f32]) -> Vec<String> {
    if v[0] <= 0.0 {
        return Vec::new();
    }
    let mut c = vec![format!(
        "aecho=in_gain={}:out_gain=0.9:delays={}:decays={}",
        n((v[4] / 100.0).clamp(0.0, 1.0)),
        n(v[1].clamp(1.0, 5000.0)),
        n((v[2] / 100.0).clamp(0.01, 1.0))
    )];
    if v[3] > 0.5 {
        c.push(format!("crossfeed=strength={}", n((v[3] / 100.0).min(1.0))));
    }
    c
}

/// Leading number of a typed value ("3.5 dB" → 3.5).
pub fn parse_num(s: &str) -> Option<f32> {
    let t = s.trim();
    let end = t
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+'))))
        .map_or(t.len(), |(i, _)| i);
    t[..end].parse().ok()
}

/// "#rgb", "rrggbb", "#RRGGBB" → "#RRGGBB".
pub fn norm_hex(s: &str) -> Option<String> {
    let h = s.trim().trim_start_matches('#');
    let h: String = if h.len() == 3 {
        h.chars().flat_map(|c| [c, c]).collect()
    } else {
        h.into()
    };
    hex(&h).map(|x| format!("#{}", &x[2..]))
}

fn swatch(h: &str) -> slint::Color {
    let v = u32::from_str_radix(h.trim_start_matches('#'), 16).unwrap_or(0xFF0000);
    slint::Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Open the side panel on Audio with the Equalizer expanded.
pub fn open_panel(a: &crate::AppWindow) {
    use slint::ComponentHandle;
    a.global::<crate::Overlays>().set_extensions_open(false);
    a.global::<crate::Overlays>().set_settings_open(false);
    a.global::<crate::Panel>().set_tab(crate::PanelTab::Audio);
    a.global::<crate::FxState>().set_eq_open(true);
    a.global::<crate::Overlays>().set_panel_open(true);
}

/// Wire the FxState global: apply saved effects now and on every change.
pub fn setup(app: &crate::AppWindow, player: &std::rc::Rc<crate::player::Player>) {
    use crate::FxState;
    use slint::ComponentHandle;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct St {
        fx: Fx,
        applied: std::collections::HashSet<String>,
    }
    let st = Rc::new(RefCell::new(St {
        fx: Fx::load(),
        applied: Default::default(),
    }));
    let floats = |v: Vec<f32>| slint::ModelRc::new(slint::VecModel::from(v));
    let strings = |v: Vec<String>| {
        slint::ModelRc::new(slint::VecModel::from(
            v.into_iter()
                .map(slint::SharedString::from)
                .collect::<Vec<_>>(),
        ))
    };
    let push = {
        let w = app.as_weak();
        let st = st.clone();
        Rc::new(move || {
            let Some(a) = w.upgrade() else { return };
            let g = a.global::<FxState>();
            let s = st.borrow();
            let fx = &s.fx;
            g.set_eq(floats(fx.get("eq")));
            g.set_video(floats(fx.get("video")));
            g.set_crop(floats(fx.get("crop")));
            g.set_color(floats(fx.get("color")));
            g.set_comp(floats(fx.get("comp")));
            g.set_reverb(floats(fx.get("reverb")));
            g.set_delay(floats(fx.get("delay")));
            g.set_preset(fx.preset.clone().into());
            g.set_auto_preamp(fx.auto_preamp);
            g.set_effective_preamp(fx.effective_preamp());
            g.set_custom(strings(fx.custom.iter().map(|c| c.name.clone()).collect()));
            g.set_builtin(strings(
                PRESETS.iter().map(|(n, _)| n.to_string()).collect(),
            ));
            g.set_extract_color(fx.extract_color.clone().into());
            g.set_threshold_color(fx.threshold_color.clone().into());
            g.set_extract_swatch(swatch(&fx.extract_color));
            g.set_threshold_swatch(swatch(&fx.threshold_color));
        })
    };
    let apply = {
        let st = st.clone();
        let p = player.clone();
        Rc::new(move |group: &str| {
            let mut s = st.borrow_mut();
            let Some((kind, label, f)) = s.fx.filter(group) else {
                return;
            };
            match f {
                Some(f) => {
                    p.cmd(kind, &["add", &format!("{label}:{f}")]);
                    s.applied.insert(label);
                }
                None => {
                    if s.applied.remove(&label) {
                        p.cmd(kind, &["remove", &label]);
                    }
                }
            }
        })
    };
    for (g, _, _) in GROUPS {
        apply(g);
    }
    push();
    let changed = {
        let st = st.clone();
        let (apply, push) = (apply.clone(), push.clone());
        Rc::new(move |group: &str| {
            if group.is_empty() {
                GROUPS.iter().for_each(|(g, _, _)| apply(g));
            } else {
                apply(group);
            }
            st.borrow().fx.save();
            push();
        })
    };
    let g = app.global::<FxState>();
    {
        let (st, changed) = (st.clone(), changed.clone());
        g.on_set(move |group, i, v| {
            let ok = st.borrow_mut().fx.set(&group, i.max(0) as usize, v);
            if ok {
                changed(&group);
            }
        });
    }
    {
        let (st, changed, push) = (st.clone(), changed.clone(), push.clone());
        g.on_set_text(move |group, i, text| {
            let ok = parse_num(&text).is_some_and(|v| {
                let v = if group == "eq" {
                    v.clamp(if i == 0 { 0.0 } else { -20.0 }, 20.0)
                } else {
                    v.max(0.0)
                };
                st.borrow_mut().fx.set(&group, i.max(0) as usize, v)
            });
            if ok {
                changed(&group);
            } else {
                push(); // restore the shown value
            }
        });
    }
    {
        let (st, changed, push) = (st.clone(), changed.clone(), push.clone());
        g.on_set_color(move |i, text| {
            let Some(h) = norm_hex(&text) else {
                push();
                return;
            };
            {
                let fx = &mut st.borrow_mut().fx;
                if i == 0 {
                    fx.extract_color = h;
                } else {
                    fx.threshold_color = h;
                }
            }
            changed("color");
        });
    }
    {
        let (st, changed) = (st.clone(), changed.clone());
        let w = app.as_weak();
        g.on_preset_action(move |action, name, arg| {
            let mut toast = None;
            {
                let fx = &mut st.borrow_mut().fx;
                match action.as_str() {
                    // Alt+E / Alt+Shift+E (keymap EqPreset / EqToggle).
                    "cycle" => toast = Some(format!("Equalizer: {}", fx.cycle_preset())),
                    "toggle" => {
                        let on = fx.toggle();
                        toast = Some(if on {
                            format!("Equalizer on: {}", fx.preset)
                        } else {
                            "Equalizer off".into()
                        });
                    }
                    "auto-preamp" => fx.auto_preamp = !fx.auto_preamp,
                    "select" => {
                        fx.select_preset(&name);
                    }
                    "new" => {
                        fx.new_preset();
                    }
                    "rename" => {
                        fx.rename_preset(&name, &arg);
                    }
                    "delete" => fx.delete_preset(&name),
                    "export" => {
                        if let Some(f) = rfd::FileDialog::new()
                            .set_title("Export equalizer presets")
                            .set_file_name("vivi-eq-presets.json")
                            .add_filter("JSON", &["json"])
                            .save_file()
                        {
                            if let Err(e) = std::fs::write(&f, fx.export_json()) {
                                eprintln!("vivi: export presets: {e}");
                            }
                        }
                    }
                    "import" => {
                        if let Some(f) = rfd::FileDialog::new()
                            .set_title("Import equalizer presets")
                            .add_filter("JSON", &["json"])
                            .pick_file()
                        {
                            let r = std::fs::read_to_string(&f)
                                .map_err(|e| e.to_string())
                                .and_then(|t| fx.import_json(&t));
                            match r {
                                Ok(names) => {
                                    fx.select_preset(&names[0]);
                                }
                                Err(e) => eprintln!("vivi: import presets: {e}"),
                            }
                        }
                    }
                    _ => {}
                }
            }
            changed("eq");
            if let (Some(t), Some(a)) = (toast, w.upgrade()) {
                crate::plugins::toast(&a, &t);
            }
        });
    }
    {
        let (st, changed) = (st.clone(), changed.clone());
        g.on_reset(move |group| {
            st.borrow_mut().fx.reset(&group);
            changed(&group);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_inactive() {
        let fx = Fx::default();
        for (g, _, _) in GROUPS {
            assert_eq!(fx.filter(g).unwrap().2, None, "{g}");
        }
    }

    #[test]
    fn eq_chain_and_presets() {
        let mut fx = Fx {
            auto_preamp: false,
            ..Default::default()
        };
        assert!(fx.select_preset("Club"));
        let (kind, label, f) = fx.filter("eq").unwrap();
        assert_eq!((kind, label.as_str()), ("af", "@vivi-eq"));
        let f = f.unwrap();
        assert!(
            f.starts_with("lavfi=[volume=-6dB,equalizer=f=310:t=o:w=1:g=8,"),
            "{f}"
        );
        fx.set("eq", 1, 3.0);
        assert_eq!(fx.preset, "Custom");
        let name = fx.new_preset();
        assert_eq!(name, "Custom Preset 01");
        fx.set("eq", 2, -4.0);
        assert_eq!(fx.custom[0].values[2], -4.0);
        assert!(fx.rename_preset(&name, "Anime"));
        assert!(!fx.rename_preset("Anime", "Flat"));
        assert_eq!(fx.preset, "Anime");
        fx.delete_preset("Anime");
        assert!(fx.custom.is_empty());
        fx.reset("eq");
        assert_eq!(fx.get("eq")[11], 1.0);
        assert_eq!(fx.filter("eq").unwrap().2, None);
    }

    #[test]
    fn auto_preamp_prevents_clipping() {
        let mut fx = Fx::default();
        assert!(fx.auto_preamp, "on by default");
        // Bass: unity preamp, +6 dB at 60 Hz → the preamp drops by 6 dB.
        fx.select_preset("Bass");
        assert_eq!(fx.effective_preamp(), -6.0);
        let f = fx.filter("eq").unwrap().2.unwrap();
        assert!(
            f.starts_with("lavfi=[volume=-6dB,equalizer=f=60:t=o:w=1:g=6,"),
            "{f}"
        );
        // Club already lowers its preamp (−6 dB) but boosts 8 dB: −8 dB, not −14.
        fx.select_preset("Club");
        assert_eq!(fx.effective_preamp(), -8.0);
        // Cuts only (Classical): nothing to compensate.
        fx.select_preset("Classical");
        assert_eq!(fx.effective_preamp(), 0.0);
        assert!(!fx.filter("eq").unwrap().2.unwrap().contains("volume="));
        fx.auto_preamp = false;
        fx.select_preset("Bass");
        assert_eq!(fx.effective_preamp(), 0.0);
        assert!(!fx.filter("eq").unwrap().2.unwrap().contains("volume="));
        // Older fx.toml without the key: on.
        let old: Fx = toml::from_str("preset = \"Rock\"\n").unwrap();
        assert!(old.auto_preamp);
        let off: Fx = toml::from_str("auto_preamp = false\n").unwrap();
        assert!(!off.auto_preamp);
    }

    #[test]
    fn plugin_presets_are_built_in() {
        for p in ["Flat", "Bass", "Vocal", "Rock", "Night"] {
            assert!(PRESETS.iter().any(|(n, _)| *n == p), "{p}");
        }
        let mut names: Vec<&str> = PRESETS.iter().map(|(n, _)| *n).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), PRESETS.len());
    }

    #[test]
    fn cycle_and_toggle() {
        let mut fx = Fx::default();
        assert_eq!(fx.get("eq")[11], 0.0);
        assert_eq!(fx.cycle_preset(), PRESETS[1].0);
        assert_eq!(fx.get("eq")[11], 1.0, "cycling turns the equalizer on");
        fx.new_preset();
        let last = fx.preset.clone();
        fx.select_preset(PRESETS[PRESETS.len() - 1].0);
        assert_eq!(
            fx.cycle_preset(),
            last,
            "custom presets follow the built-in ones"
        );
        assert_eq!(fx.cycle_preset(), "Flat", "wraps around");
        fx.set("eq", 3, 1.0);
        assert_eq!(fx.preset, "Custom");
        assert_eq!(
            fx.cycle_preset(),
            "Flat",
            "Custom starts at the first preset"
        );
        assert!(!fx.toggle());
        assert_eq!(fx.get("eq")[11], 0.0);
        assert_eq!(fx.preset, "Flat", "toggling keeps the preset");
        assert!(fx.toggle());
        assert_eq!(fx.get("eq")[11], 1.0);
    }

    #[test]
    fn crop_link_and_video() {
        let mut fx = Fx::default();
        fx.set("crop", 4, 1.0);
        fx.set("crop", 0, 10.0);
        assert_eq!(
            fx.filter("crop").unwrap().2.unwrap(),
            "lavfi=[crop=w=iw-20:h=ih-20:x=10:y=10]"
        );
        fx.set("video", 0, 90.0);
        fx.set("video", 7, 20.0);
        assert_eq!(
            fx.filter("video").unwrap().2.unwrap(),
            "lavfi=[hue=h=90,noise=alls=20:allf=t]"
        );
    }

    #[test]
    fn audio_effects() {
        let mut fx = Fx::default();
        fx.set("comp", 0, 1.0);
        let c = fx.filter("comp").unwrap().2.unwrap();
        assert!(
            c.contains("acompressor=detection=peak:attack=25:release=100"),
            "{c}"
        );
        fx.set("reverb", 0, 1.0);
        assert!(fx.filter("reverb").unwrap().2.unwrap().contains("aecho="));
        fx.set("delay", 0, 1.0);
        assert!(fx
            .filter("delay")
            .unwrap()
            .2
            .unwrap()
            .contains("crossfeed=strength=0.3"));
        fx.set("color", 8, 1.0);
        fx.extract_color = "bad".into();
        fx.set("color", 0, 1.0);
        let c = fx.filter("color").unwrap().2.unwrap();
        assert!(c.starts_with("lavfi=[colorchannelmixer=rr=0.393:"), "{c}");
    }

    #[test]
    fn json_roundtrip() {
        let mut a = Fx::default();
        a.select_preset("Rock");
        let one = a.export_json();
        assert!(one.contains("\"vivi-eq-presets\""));
        let mut b = Fx::default();
        assert_eq!(b.import_json(&one).unwrap(), ["Rock (2)"]);
        let rock = PRESETS.iter().find(|(n, _)| *n == "Rock").unwrap().1;
        assert_eq!(b.custom[0].values, rock.to_vec());
        b.custom[0].name = "Mine".into();
        let mut c = Fx::default();
        c.import_json(&b.export_json()).unwrap();
        c.import_json(&b.export_json()).unwrap();
        assert_eq!(c.custom[1].name, "Mine (2)");
        let bare = r#"[{"name": "Loud", "bands": [30,0,0,0,0,0,0,0,0,0]}]"#;
        assert_eq!(c.import_json(bare).unwrap(), ["Loud"]);
        assert_eq!(c.custom[2].values[..2], [12.0, 20.0]);
        assert!(c.import_json(r#"{"name":"x","bands":[1,2]}"#).is_err());
        assert!(c.import_json("nope").is_err());
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_num(" 3.5 dB"), Some(3.5));
        assert_eq!(parse_num("-12"), Some(-12.0));
        assert_eq!(parse_num("dB"), None);
        assert_eq!(norm_hex("f0a").as_deref(), Some("#FF00AA"));
        assert_eq!(norm_hex("#00ff00").as_deref(), Some("#00FF00"));
        assert_eq!(norm_hex("#zz0000"), None);
    }

    #[test]
    fn normalize_and_roundtrip() {
        let mut fx: Fx = toml::from_str("preset = \"Rock\"\n[groups]\neq = [1.0]\n").unwrap();
        fx.normalize();
        assert_eq!(fx.get("eq").len(), 12);
        assert_eq!(fx.get("eq")[0], 1.0);
        let s = toml::to_string_pretty(&fx).unwrap();
        assert_eq!(toml::from_str::<Fx>(&s).unwrap(), fx);
    }
}
