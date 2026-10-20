use std::process::Command;

fn main() {
    // Build info for Settings → About.
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let hash = std::env::var("VIVI_GIT_HASH")
        .ok()
        .or_else(|| git(&["rev-parse", "--short", "HEAD"]))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=VIVI_GIT_HASH={hash}");
    // Date only (UTC), from SOURCE_DATE_EPOCH for reproducible builds.
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
    println!("cargo:rustc-env=VIVI_BUILD_DATE={}", ymd(secs / 86400));
    println!(
        "cargo:rustc-env=VIVI_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
    let slint = std::fs::read_to_string("Cargo.lock")
        .ok()
        .and_then(|l| {
            let i = l.find("name = \"slint\"\nversion = \"")?;
            let rest = &l[i + 26..];
            Some(rest[..rest.find('"')?].to_string())
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=VIVI_SLINT_VERSION={slint}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-env-changed=VIVI_GIT_HASH");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-changed=ui");

    // Dark std-widgets (LineEdit, ScrollView) to match the Figma dark theme.
    // The Slint compiler recurses through long `if` chains (ui/core/duo.slint has one entry per
    // icon); Windows gives the main thread only 1 MB of stack, so compile on a thread with more.
    std::thread::Builder::new()
        .name("slint-build".into())
        .stack_size(256 * 1024 * 1024)
        .spawn(|| {
            let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
            slint_build::compile_with_config("ui/app.slint", config)
                .expect("failed to compile Slint UI");
        })
        .expect("failed to start the Slint compiler thread")
        .join()
        .expect("failed to compile Slint UI");
}

/// Days since 1970-01-01 → "YYYY-MM-DD" (civil-from-days).
fn ymd(days: u64) -> String {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}
