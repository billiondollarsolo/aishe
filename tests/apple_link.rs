//! Capability checks inspect emitted load commands, not linker option names.
#[allow(dead_code)]
#[path = "../build_support/apple_link.rs"]
mod apple_link;

#[test]
fn outliner_policy_requires_unambiguous_inherited_disable_flag() {
    for encoded in [
        "-C\u{1f}llvm-args=-enable-machine-outliner=never",
        "-Cllvm-args=-enable-machine-outliner=never",
        "--codegen=llvm-args=--enable-machine-outliner=never",
        "-C\u{1f}llvm-args=-other-option=1 -enable-machine-outliner=never",
    ] {
        assert!(apple_link::machine_outliner_disabled(encoded), "{encoded}");
    }
    for encoded in [
        "", "-D\u{1f}warnings", "-C\u{1f}llvm-args=-enable-machine-outliner=always",
        "-C\u{1f}llvm-args=-enable-machine-outliner=never -enable-machine-outliner=always",
        "-C\u{1f}llvm-args=-enable-machine-outliner=always -enable-machine-outliner=never",
        "-C\u{1f}llvm-args=-enable-machine-outliner=never\u{1f}-C\u{1f}llvm-args=-enable-machine-outliner",
        "-C\u{1f}llvm-args=-enable-machine-outliner=never\u{1f}-C\u{1f}llvm-args=-enable-machine-outliner=default",
        "-C\u{1f}llvm-args=-enable-machine-outliner=never\u{1f}-Cllvm-args=-enable-machine-outliner=always",
        "-C\u{1f}llvm-args=-enable-machine-outliner=never\u{1f}-C\u{1f}llvm-args=-enable-machine-outliner-mode=unknown",
    ] {
        assert!(!apple_link::machine_outliner_disabled(encoded), "{encoded}");
    }
}

#[test]
fn capability_probe_uses_optimized_release_profile_and_debug_level() {
    assert_eq!(
        apple_link::profile_codegen_args("release", "z"),
        [
            "-C",
            "opt-level=z",
            "-C",
            "lto=fat",
            "-C",
            "codegen-units=1"
        ]
    );
    assert_eq!(
        apple_link::profile_codegen_args("debug", "0"),
        ["-C", "opt-level=0"]
    );
}

fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn dependency(name: &str, delayed: bool) -> Vec<u8> {
    let path = format!("/System/Library/Frameworks/{name}.framework/Versions/A/{name}");
    let name_offset = if delayed { 28 } else { 24 };
    let mut bytes = vec![0; (name_offset + path.len() + 1 + 7) & !7];
    let size = bytes.len() as u32;
    put(&mut bytes, 0, 0xc);
    put(&mut bytes, 4, size);
    put(&mut bytes, 8, name_offset as u32);
    if delayed {
        put(&mut bytes, 12, 0x1a74_1800);
        put(&mut bytes, 24, 8);
    }
    bytes[name_offset..name_offset + path.len()].copy_from_slice(path.as_bytes());
    bytes
}

fn image(min_os: u32, dependencies: Vec<Vec<u8>>, pointer_format: Option<u16>) -> Vec<u8> {
    let mut version = vec![0; 24];
    put(&mut version, 0, 0x32);
    put(&mut version, 4, 24);
    put(&mut version, 8, 1);
    put(&mut version, 12, min_os);
    let mut commands = vec![version];
    commands.extend(dependencies);
    let mut chain_payload = Vec::new();
    if let Some(format) = pointer_format {
        let mut chains = vec![0; 16];
        put(&mut chains, 0, 0x8000_0034);
        put(&mut chains, 4, 16);
        put(&mut chains, 12, 60);
        commands.push(chains);
        chain_payload = vec![0; 60];
        put(&mut chain_payload, 4, 28); // starts offset
        put(&mut chain_payload, 8, 60); // imports offset
        put(&mut chain_payload, 12, 60); // symbols offset
        put(&mut chain_payload, 20, 1); // imports format
        put(&mut chain_payload, 28, 1); // one segment
        put(&mut chain_payload, 32, 8); // relative segment offset
        put(&mut chain_payload, 36, 24); // segment size
        chain_payload[40..42].copy_from_slice(&0x4000u16.to_le_bytes());
        chain_payload[42..44].copy_from_slice(&format.to_le_bytes());
        chain_payload[56..58].copy_from_slice(&1u16.to_le_bytes());
    }
    let command_size: usize = commands.iter().map(Vec::len).sum();
    if pointer_format.is_some() {
        let last = commands.last_mut().unwrap();
        put(last, 8, (32 + command_size) as u32);
    }
    let mut bytes = vec![0; 32];
    put(&mut bytes, 0, 0xfeed_facf);
    put(&mut bytes, 4, 0x0100_000c);
    put(&mut bytes, 16, commands.len() as u32);
    put(&mut bytes, 20, command_size as u32);
    for command in commands {
        bytes.extend(command);
    }
    bytes.extend(chain_payload);
    bytes
}

#[test]
fn delayed_capability_requires_both_actual_marked_dependencies() {
    for delayed in [false, true] {
        let bytes = image(
            11 << 16,
            vec![
                dependency("CoreFoundation", delayed),
                dependency("Security", delayed),
            ],
            None,
        );
        assert_eq!(
            apple_link::parse_macho(&bytes)
                .unwrap()
                .delayed_frameworks(),
            delayed
        );
    }
    let missing = image(11 << 16, vec![dependency("Security", true)], None);
    assert!(!apple_link::parse_macho(&missing)
        .unwrap()
        .delayed_frameworks());
    let mixed = image(
        11 << 16,
        vec![
            dependency("CoreFoundation", true),
            dependency("Security", false),
        ],
        None,
    );
    assert!(!apple_link::parse_macho(&mixed)
        .unwrap()
        .delayed_frameworks());
}

#[test]
fn ordinary_duplicate_prevents_delayed_admission_in_either_order() {
    for ordinary_first in [false, true] {
        let mut dependencies = vec![
            dependency("CoreFoundation", true),
            dependency("Security", true),
        ];
        if ordinary_first {
            dependencies.insert(0, dependency("Security", false));
        } else {
            dependencies.push(dependency("Security", false));
        }
        assert!(
            !apple_link::parse_macho(&image(11 << 16, dependencies, None))
                .unwrap()
                .delayed_frameworks()
        );
    }
}

#[test]
fn marker_and_delayed_bit_are_both_required() {
    for field in [12, 24] {
        let mut security = dependency("Security", true);
        put(&mut security, field, 0);
        let bytes = image(
            11 << 16,
            vec![dependency("CoreFoundation", true), security],
            None,
        );
        assert!(!apple_link::parse_macho(&bytes)
            .unwrap()
            .delayed_frameworks());
    }
}

#[test]
fn chain_formats_preserve_arm64_deployment_floor_and_x86_fallback() {
    for (major, format, expected) in [
        (10, 2, false),
        (11, 2, true),
        (11, 6, false),
        (12, 6, true),
        (12, 2, true),
        (12, 9, false),
    ] {
        let bytes = image(major << 16, vec![], Some(format));
        assert_eq!(
            apple_link::parse_macho(&bytes).unwrap().compatible_chains(),
            expected
        );
    }
    let mut x86 = image(11 << 16, vec![], Some(2));
    put(&mut x86, 4, 0x0100_0007);
    assert!(!apple_link::parse_macho(&x86).unwrap().compatible_chains());
}

#[test]
fn capability_candidate_must_preserve_baseline_architecture_and_minimum() {
    let baseline = apple_link::parse_macho(&image(11 << 16, vec![], None)).unwrap();
    let matching = apple_link::parse_macho(&image(11 << 16, vec![], Some(2))).unwrap();
    assert!(matching.same_deployment(&baseline));
    let raised = apple_link::parse_macho(&image(12 << 16, vec![], Some(6))).unwrap();
    assert!(!raised.same_deployment(&baseline));
    let mut x86 = image(11 << 16, vec![], None);
    put(&mut x86, 4, 0x0100_0007);
    assert!(!apple_link::parse_macho(&x86)
        .unwrap()
        .same_deployment(&baseline));
}

#[test]
fn chain_header_and_import_limits_must_match_older_decoder() {
    for (field, value) in [(0, 1), (16, 0xffff), (20, 4), (24, 1)] {
        let mut bytes = image(11 << 16, vec![], Some(2));
        let payload = 32 + u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
        put(&mut bytes, payload + field, value);
        assert!(!apple_link::parse_macho(&bytes).unwrap().compatible_chains());
    }
}

#[test]
fn malformed_or_truncated_commands_never_enable_capabilities() {
    let bytes = image(
        11 << 16,
        vec![
            dependency("CoreFoundation", true),
            dependency("Security", true),
        ],
        Some(2),
    );
    for length in 0..bytes.len() {
        assert!(
            apple_link::parse_macho(&bytes[..length]).is_none(),
            "accepted truncation at {length}"
        );
    }
    let mut oversized = bytes.clone();
    put(&mut oversized, 36, u32::MAX);
    assert!(apple_link::parse_macho(&oversized).is_none());
    let mut missing_version = bytes;
    put(&mut missing_version, 32, 0);
    assert!(apple_link::parse_macho(&missing_version).is_none());
}
