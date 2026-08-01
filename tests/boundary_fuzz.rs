//! Deterministic boundary fuzzing for parsers that sit between untrusted text,
//! terminal output, shell handoffs, and public JSON. A failing seed belongs in a
//! named regression fixture before this corpus changes.

use std::path::PathBuf;

use aishe::dispatcher::CommandCache;

const SEEDS: [u64; 4] = [
    0x4149_5348_455f_0001,
    0x4149_5348_455f_0002,
    0x9e37_79b9_7f4a_7c15,
    0xd1b5_4a32_d192_ed03,
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn bytes(&mut self, maximum: usize) -> Vec<u8> {
        let length = (self.next() as usize) % maximum.max(1);
        (0..length).map(|_| self.next() as u8).collect()
    }

    fn text(&mut self, maximum: usize) -> String {
        const HOSTILE: &[char] = &[
            '\0', '\u{1b}', '\r', '\n', '\t', '\u{7f}', '\u{85}', '\u{202e}', '\u{2066}', ' ', '?',
            '!', '#', '/', '.', ';', '|', '&', '$', '`', '\'', '"', 'é', '界', '🚀',
        ];
        let length = (self.next() as usize) % maximum.max(1);
        let mut value = String::new();
        for _ in 0..length {
            if self.next().is_multiple_of(5) {
                value.push(HOSTILE[(self.next() as usize) % HOSTILE.len()]);
            } else {
                value.push(char::from(32 + (self.next() % 95) as u8));
            }
        }
        value
    }
}

fn contains_unsafe_display_character(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(
            character,
            '\0'..='\u{8}'
                | '\u{b}'..='\u{1f}'
                | '\u{7f}'..='\u{9f}'
                | '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
    })
}

#[test]
fn route_safety_terminal_and_error_boundaries_are_total_for_fixed_seeds() {
    let cache = CommandCache::new();
    cache.insert_all(&["echo", "find", "install", "git", "what", "where"]);
    for seed in SEEDS {
        let mut rng = Rng(seed);
        for case in 0..2_000 {
            let input = rng.text(512);
            let first = aishe::dispatcher::route(&input, &cache);
            let second = aishe::dispatcher::route(&input, &cache);
            assert_eq!(
                first, second,
                "route nondeterminism at seed {seed:#x}/{case}"
            );
            let diagnostic = serde_json::to_string(&first.diagnostic()).unwrap();
            assert!(diagnostic.len() <= 12_000);
            assert!(!diagnostic.contains('\u{1b}'));

            let risk_a = aishe::safety::assess(&input);
            let risk_b = aishe::safety::assess(&input);
            assert_eq!(
                risk_a, risk_b,
                "safety nondeterminism at seed {seed:#x}/{case}"
            );

            let safe = aishe::commands::display_safe_multiline(&input);
            assert!(
                !contains_unsafe_display_character(&safe),
                "unsafe display character at seed {seed:#x}/{case}: {safe:?}"
            );
            for width in [20, 40, 80, 120, 200] {
                for line in aishe::ui::wrap_cells(&safe, width) {
                    assert!(aishe::ui::cell_width(&line) <= width);
                }
            }

            let source = std::io::Error::other(input);
            let public = aishe::user_error::UserError::from_error(&source);
            let json = public.render_json().unwrap();
            assert!(serde_json::from_str::<aishe::user_error::UserError>(&json).is_ok());
            assert!(!json.contains('\u{1b}'));
        }
    }
}

#[test]
fn shell_selection_reader_rejects_arbitrary_bytes_without_escape_or_panic() {
    let root = std::env::temp_dir().join(format!(
        "aishe-boundary-fuzz-selection-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    for seed in SEEDS {
        let mut rng = Rng(seed);
        for case in 0..256 {
            let path: PathBuf = root.join(format!("{seed:016x}-{case}.selection"));
            std::fs::write(&path, rng.bytes(6_000)).unwrap();
            let _ = aishe::connection::read_selection(&path);
            std::fs::remove_file(path).unwrap();
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn oversized_route_diagnostic_remains_bounded() {
    let input = format!("? {}", "界\u{1b}[2J".repeat(100_000));
    let decision = aishe::dispatcher::route(&input, &CommandCache::new());
    let json = serde_json::to_string(&decision.diagnostic()).unwrap();
    assert!(json.len() <= 12_000);
    assert!(!json.contains('\u{1b}'));
}
