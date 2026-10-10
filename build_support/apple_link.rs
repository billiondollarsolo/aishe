//! Enable Apple loader optimizations only when the selected toolchain emits
//! compatible Mach-O commands. Unsupported SDKs/linkers keep ordinary linking.

use std::{env, fs, path::Path, process::Command};

const DELAY_FLAGS: [&str; 2] = [
    "-Wl,-delay_framework,CoreFoundation",
    "-Wl,-delay_framework,Security",
];
const CHAIN_FLAG: &str = "-Wl,-fixup_chains";
const MACOS_11: u32 = 11 << 16;
const MACOS_12: u32 = 12 << 16;
const ARM64: u32 = 0x0100_000c;

// Ordinary framework imports deliberately coexist with the delayed arguments:
// flag acceptance alone does not establish how the linker merges duplicates.
const PROBE_SOURCE: &str = r#"
use std::ffi::c_void;
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFGetTypeID(value: *const c_void) -> usize;
    static kCFAllocatorDefault: *const c_void;
}
#[link(name = "Security", kind = "framework")]
extern "C" { fn SecPolicyCreateSSL(server: u8, name: *const c_void) -> *const c_void; }
fn main() {
    // This executable is inspected, never run.
    unsafe { std::hint::black_box(CFGetTypeID(std::ptr::null()));
             std::hint::black_box(kCFAllocatorDefault);
             std::hint::black_box(SecPolicyCreateSSL(0, std::ptr::null())); }
}
"#;

pub fn configure() {
    println!("cargo:rustc-check-cfg=cfg(aishe_delayed_frameworks)");
    println!("cargo:rerun-if-changed=build_support/apple_link.rs");
    println!("cargo:rerun-if-changed=.cargo/config.toml");
    for name in [
        "RUSTC",
        "RUSTC_LINKER",
        "CARGO_ENCODED_RUSTFLAGS",
        "SDKROOT",
        "MACOSX_DEPLOYMENT_TARGET",
        "OPT_LEVEL",
        "PROFILE",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let target = env::var("TARGET").unwrap_or_default();
    let encoded_flags = env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    let outliner_disabled = machine_outliner_disabled(&encoded_flags);
    let opt_level = env::var("OPT_LEVEL").unwrap_or_else(|_| "0".to_string());
    let out = env::var_os("OUT_DIR");
    let mut flags = Vec::new();
    let mut baseline_min = None;
    let mut delayed = false;
    let mut chained = false;
    let mut delayed_status = "not-applicable";
    let mut chained_status = "not-applicable";
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        delayed_status = "default-probe-failed";
        chained_status = "default-probe-failed";
        if let Some(out) = out.as_deref() {
            let out = Path::new(out);
            if let Some(base) = probe(out, &target, "default", &[]) {
                baseline_min = Some(base.min_os);
                delayed_status = "unsupported";
                if base.cpu_type == ARM64 && !outliner_disabled {
                    // Cargo environment/target overrides may omit repository
                    // flags. An optimized small probe cannot rule out outlining
                    // across every dependency in the final LTO image.
                    delayed_status = "outliner-policy-missing";
                } else if let Some(candidate) = probe(out, &target, "delayed", &DELAY_FLAGS) {
                    if candidate.same_deployment(&base) && candidate.delayed_frameworks() {
                        delayed = true;
                        delayed_status = "verified";
                        flags.extend(DELAY_FLAGS);
                    } else if !candidate.same_deployment(&base) {
                        delayed_status = "deployment-mismatch";
                    }
                }
                // x86_64 retains its existing 10.12-compatible fixups. At the
                // arm64 11.0 floor, require the older generic64 chain encoding.
                chained_status = "not-applicable";
                if base.cpu_type == ARM64 && base.min_os >= MACOS_11 {
                    chained_status = "unsupported";
                    let mut candidate_flags = flags.clone();
                    candidate_flags.push(CHAIN_FLAG);
                    if let Some(candidate) = probe(out, &target, "chained", &candidate_flags) {
                        if candidate.same_deployment(&base)
                            && candidate.compatible_chains()
                            && (!delayed || candidate.delayed_frameworks())
                        {
                            chained = true;
                            chained_status = "verified";
                            flags.push(CHAIN_FLAG);
                        } else if !candidate.same_deployment(&base) {
                            chained_status = "deployment-mismatch";
                        }
                    }
                }
            }
        }
    }
    for flag in &flags {
        println!("cargo:rustc-link-arg={flag}");
    }
    if delayed {
        println!("cargo:rustc-cfg=aishe_delayed_frameworks");
    }
    let flags_json = flags
        .iter()
        .map(|flag| format!("\"{flag}\""))
        .collect::<Vec<_>>()
        .join(",");
    let min_json = baseline_min.map_or_else(|| "null".to_string(), |min| min.to_string());
    let source_commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|commit| commit.trim().to_string())
        .filter(|commit| commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown".to_string());
    let record = format!(
        "{{\"schema_version\":1,\"source_commit\":\"{source_commit}\",\"target\":\"{target}\",\"baseline_min_os\":{min_json},\"probe_opt_level\":\"{opt_level}\",\"machine_outliner_disabled\":{outliner_disabled},\"delayed_frameworks\":{delayed},\"chained_fixups\":{chained},\"flags\":[{flags_json}],\"delayed_status\":\"{delayed_status}\",\"chained_status\":\"{chained_status}\"}}"
    );
    println!("cargo:rustc-env=AISHE_STARTUP_LINK_DIAGNOSTIC={record}");
    if let Some(out) = out {
        let _ = fs::write(Path::new(&out).join("startup-link.json"), record);
    }
}

fn probe(out: &Path, target: &str, name: &str, flags: &[&str]) -> Option<MachO> {
    let source = out.join("apple-link-probe.rs");
    fs::write(&source, PROBE_SOURCE).ok()?;
    let binary = out.join(format!("apple-link-probe-{name}"));
    let mut command = Command::new(env::var_os("RUSTC")?);
    command.args([
        "--crate-name",
        "aishe_apple_link_probe",
        "--edition=2021",
        "--target",
        target,
    ]);
    command.arg(&source).arg("-o").arg(&binary);
    let opt_level = env::var("OPT_LEVEL").unwrap_or_else(|_| "0".to_string());
    command.args(profile_codegen_args(
        &env::var("PROFILE").unwrap_or_default(),
        &opt_level,
    ));
    if let Some(linker) = env::var_os("RUSTC_LINKER") {
        command
            .arg("-C")
            .arg(format!("linker={}", linker.to_string_lossy()));
    }
    if let Ok(encoded) = env::var("CARGO_ENCODED_RUSTFLAGS") {
        command.args(encoded.split('\u{1f}').filter(|arg| !arg.is_empty()));
    }
    for flag in flags {
        command.arg("-C").arg(format!("link-arg={flag}"));
    }
    if !command.output().ok()?.status.success() {
        return None;
    }
    parse_macho(&fs::read(binary).ok()?)
}

pub fn profile_codegen_args(profile: &str, opt_level: &str) -> Vec<String> {
    let mut args = vec!["-C".to_string(), format!("opt-level={opt_level}")];
    if profile == "release" {
        // Match this repository's optimized release profile. User rustflags
        // follow these defaults, just as they do in Cargo's rustc invocation.
        args.extend(["-C", "lto=fat", "-C", "codegen-units=1"].map(str::to_string));
    }
    args
}

pub fn machine_outliner_disabled(encoded: &str) -> bool {
    let mut disabled = false;
    let mut codegen_next = false;
    for argument in encoded.split('\u{1f}') {
        let option = if codegen_next {
            Some(argument)
        } else {
            argument
                .strip_prefix("-C")
                .or_else(|| argument.strip_prefix("--codegen="))
        };
        codegen_next = argument == "-C" || argument == "--codegen";
        let Some(value) = option.and_then(|option| option.strip_prefix("llvm-args=")) else {
            continue;
        };
        for llvm_argument in value.split_whitespace() {
            let option = llvm_argument.trim_start_matches('-');
            if option == "enable-machine-outliner=never" {
                disabled = true;
            } else if option.starts_with("enable-machine-outliner") {
                // Contradictory or implicit-enable occurrences fail closed,
                // regardless of LLVM's ordering rules for repeated options.
                return false;
            }
        }
    }
    disabled
}

#[derive(Debug, PartialEq, Eq)]
pub struct MachO {
    pub cpu_type: u32,
    pub min_os: u32,
    pub delayed: [bool; 2],
    pub chains: Option<Chains>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Chains {
    pub version: u32,
    pub imports_format: u32,
    pub imports_count: u32,
    pub symbols_format: u32,
    pub pointer_formats: Vec<u16>,
}

impl MachO {
    pub fn same_deployment(&self, baseline: &Self) -> bool {
        self.cpu_type == baseline.cpu_type && self.min_os == baseline.min_os
    }

    pub fn delayed_frameworks(&self) -> bool {
        self.delayed == [true, true]
    }

    pub fn compatible_chains(&self) -> bool {
        let Some(chains) = &self.chains else {
            return false;
        };
        self.cpu_type == ARM64
            && self.min_os >= MACOS_11
            && chains.version == 0
            && matches!(chains.imports_format, 1..=3)
            && chains.imports_count < 0xffff
            && chains.symbols_format == 0
            && !chains.pointer_formats.is_empty()
            && chains
                .pointer_formats
                .iter()
                .all(|format| *format == 2 || (self.min_os >= MACOS_12 && *format == 6))
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

pub fn parse_macho(bytes: &[u8]) -> Option<MachO> {
    // The supported Darwin targets emit thin, little-endian 64-bit Mach-O.
    if u32_at(bytes, 0)? != 0xfeed_facf {
        return None;
    }
    let cpu_type = u32_at(bytes, 4)?;
    let commands = u32_at(bytes, 16)?;
    let end = 32usize.checked_add(u32_at(bytes, 20)? as usize)?;
    bytes.get(..end)?;
    let mut offset = 32usize;
    let mut min_os = None;
    let mut delayed = [None, None];
    let mut chains = None;
    for _ in 0..commands {
        let cmd = u32_at(bytes, offset)?;
        let size = u32_at(bytes, offset + 4)? as usize;
        if size < 8 {
            return None;
        }
        let next = offset.checked_add(size)?;
        if next > end {
            return None;
        }
        let command = bytes.get(offset..next)?;
        match cmd {
            0x32 if u32_at(command, 8)? == 1 => {
                if min_os.replace(u32_at(command, 12)?).is_some() {
                    return None;
                }
            }
            0x24 => {
                if min_os.replace(u32_at(command, 8)?).is_some() {
                    return None;
                }
            }
            0xc | 0x8000_0018 | 0x8000_001f | 0x8000_0023 => {
                let name_offset = u32_at(command, 8)? as usize;
                if name_offset < 24 {
                    return None;
                }
                let name = command.get(name_offset..)?;
                let name =
                    std::str::from_utf8(name.get(..name.iter().position(|b| *b == 0)?)?).ok()?;
                for (index, framework) in ["CoreFoundation", "Security"].iter().enumerate() {
                    if name.ends_with(&format!("/{framework}.framework/Versions/A/{framework}")) {
                        let is_delayed = cmd == 0xc
                            && name_offset >= 28
                            && u32_at(command, 12)? == 0x1a74_1800
                            && u32_at(command, 24)? & 8 != 0;
                        // Every matching dependency must carry the delayed bit.
                        delayed[index] = Some(delayed[index].unwrap_or(true) && is_delayed);
                    }
                }
            }
            0x8000_0034 => {
                if chains.is_some() {
                    return None;
                }
                let start = u32_at(command, 8)? as usize;
                let end = start.checked_add(u32_at(command, 12)? as usize)?;
                chains = Some(parse_chains(bytes.get(start..end)?)?);
            }
            _ => {}
        }
        offset = next;
    }
    if offset != end {
        return None;
    }
    Some(MachO {
        cpu_type,
        min_os: min_os?,
        delayed: delayed.map(|value| value.unwrap_or(false)),
        chains,
    })
}

fn parse_chains(bytes: &[u8]) -> Option<Chains> {
    let starts = u32_at(bytes, 4)? as usize;
    let count = u32_at(bytes, starts)? as usize;
    let mut pointer_formats = Vec::new();
    for index in 0..count {
        let field = starts.checked_add(4)?.checked_add(index.checked_mul(4)?)?;
        let relative = u32_at(bytes, field)? as usize;
        if relative == 0 {
            continue;
        }
        let segment = starts.checked_add(relative)?;
        let size = u32_at(bytes, segment)? as usize;
        if size < 22 {
            return None;
        }
        let segment_bytes = bytes.get(segment..segment.checked_add(size)?)?;
        let format = u16::from_le_bytes(segment_bytes.get(6..8)?.try_into().ok()?);
        pointer_formats.push(format);
    }
    Some(Chains {
        version: u32_at(bytes, 0)?,
        imports_count: u32_at(bytes, 16)?,
        imports_format: u32_at(bytes, 20)?,
        symbols_format: u32_at(bytes, 24)?,
        pointer_formats,
    })
}
