//! Release-mode probe for pure route, picker, and long-answer rendering work.
//!
//! This is an example target rather than another shipped binary. The Python
//! performance harness builds it with the same default/no-highlight feature set
//! as the corresponding AIShe binary and redirects rendered output to a sink.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use aishe::dispatcher::{self, CommandCache};
use serde_json::{json, Value};

const SCHEMA_VERSION: u32 = 1;

fn percentile(values: &[f64], rank: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(f64::total_cmp);
    let position = (ordered.len() - 1) as f64 * rank / 100.0;
    let lower = position.floor() as usize;
    let upper = (lower + 1).min(ordered.len() - 1);
    ordered[lower] * (1.0 - position.fract()) + ordered[upper] * position.fract()
}

fn samples(mut workload: impl FnMut() -> usize, count: usize, batch: usize) -> (Vec<f64>, usize) {
    let mut timings = Vec::with_capacity(count);
    let mut checksum = 0_usize;
    for _ in 0..count {
        let started = Instant::now();
        for _ in 0..batch {
            checksum = checksum.wrapping_add(black_box(workload()));
        }
        timings.push(started.elapsed().as_secs_f64() * 1000.0 / batch as f64);
    }
    (timings, checksum)
}

fn timed_metric(values: &[f64], classification: &str, max_p95_ms: Option<f64>) -> Value {
    let p50 = percentile(values, 50.0);
    let p95 = percentile(values, 95.0);
    let minimum = values.iter().copied().reduce(f64::min).unwrap_or(0.0);
    let maximum = values.iter().copied().reduce(f64::max).unwrap_or(0.0);
    let mut metric = json!({
        "classification": classification,
        "samples": values.len(),
        "min_ms": minimum,
        "p50_ms": p50,
        "p95_ms": p95,
        "max_ms": maximum,
    });
    if let Some(limit) = max_p95_ms {
        metric["threshold"] = json!({
            "statistic": "p95_ms",
            "operator": "<=",
            "value_ms": limit,
            "pass": p95 <= limit,
        });
    }
    metric
}

fn long_answer() -> String {
    let section = "## Repository observation\n\n```bash\nprintf 'performance fixture %04d\\n' 42\nfor item in alpha beta gamma; do echo \"$item\"; done\n```\n\n";
    let prose = "The route remains local, deterministic, and bounded. **No provider request is needed.** The fixture retains paragraphs, lists, Unicode width, and fenced code while avoiding an unrealistic fence on every few lines.\n\n- one stable item\n- another stable item\n\n";
    let mut text = String::with_capacity(72 * 1024);
    while text.len() < 64 * 1024 {
        text.push_str(section);
        for _ in 0..16 {
            text.push_str(prose);
        }
    }
    text
}

fn arguments() -> (PathBuf, usize) {
    let mut output = None;
    let mut count = 60_usize;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--output" => output = args.next().map(PathBuf::from),
            "--samples" => {
                count = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0)
            }
            _ => panic!("unknown argument: {argument}"),
        }
    }
    let output = output.expect("--output PATH is required");
    assert!(count >= 20, "--samples must be at least 20");
    (output, count)
}

fn main() {
    let (output, sample_count) = arguments();

    let cache = CommandCache::new();
    cache.insert_all(&["git", "printf", "ls", "cargo", "docker"]);
    let route_inputs = [
        "git status --short",
        "what changed in this repository?",
        "printf 'hello\\n'",
        "? explain the current branch",
        "!cargo test --locked",
        "/status",
        "FOO=bar cargo check",
        "for item in a b; do echo $item; done",
    ];
    for input in route_inputs {
        black_box(dispatcher::route(input, &cache).diagnostic());
    }
    let mut route_index = 0_usize;
    let (route_times, route_checksum) = samples(
        || {
            let decision =
                dispatcher::route(route_inputs[route_index % route_inputs.len()], &cache);
            route_index += 1;
            black_box(
                decision.normalized.len() + decision.head.as_deref().map(str::len).unwrap_or(0),
            )
        },
        sample_count,
        100,
    );

    let options = (0..1000)
        .map(|index| {
            format!(
                "model-{index:04} · deterministic-provider-{index:04} · provider/model-{index:04}"
            )
        })
        .collect::<Vec<_>>();
    let (ranking_times, ranking_checksum) = samples(
        || {
            let matches = aishe::promptui::performance_picker_matches(&options, "m099");
            black_box(
                matches
                    .iter()
                    .fold(matches.len(), |sum, index| sum.wrapping_add(*index)),
            )
        },
        sample_count,
        1,
    );
    let all_matches = aishe::promptui::performance_picker_matches(&options, "model");
    let mut selected = 0_usize;
    let (frame_times, frame_checksum) = samples(
        || {
            selected = (selected + 37) % all_matches.len();
            let lines =
                aishe::promptui::performance_picker_frame(&options, &all_matches, selected, 20);
            black_box(lines.iter().map(String::len).sum::<usize>())
        },
        sample_count,
        100,
    );

    let answer = long_answer();
    let render_cold_started = Instant::now();
    let rendered_bytes = aishe::modes::performance_render_long_answer(&answer);
    let render_cold_ms = render_cold_started.elapsed().as_secs_f64() * 1000.0;
    // Rendering is informational and emits the full fixture to the caller's
    // sink. Five warm samples are enough to expose syntax/theme initialization
    // separately from steady state without turning a local gate into a soak.
    let render_samples = 5;
    let (render_times, render_checksum) = samples(
        || aishe::modes::performance_render_long_answer(black_box(&answer)),
        render_samples,
        1,
    );

    let route = timed_metric(&route_times, "enforced", Some(1.0));
    let picker_ranking = timed_metric(&ranking_times, "enforced", Some(25.0));
    let picker_frame = timed_metric(&frame_times, "enforced", Some(1.0));
    let long_answer = timed_metric(&render_times, "informational", None);
    let thresholds_pass = [&route, &picker_ranking, &picker_frame]
        .iter()
        .all(|metric| metric["threshold"]["pass"].as_bool() == Some(true));
    let report = json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "aishe_pure_performance",
        "feature_set": if cfg!(feature = "highlight") { "default" } else { "no_highlight" },
        "thresholds_pass": thresholds_pass,
        "route_decision": route,
        "picker_1000_rows": {
            "rows": options.len(),
            "visible_rows": 20,
            "ranking": picker_ranking,
            "pure_frame_redraw": picker_frame,
        },
        "long_answer_render": {
            "input_bytes": answer.len(),
            "rendered_input_bytes": rendered_bytes,
            "cold_ms": render_cold_ms,
            "warm": long_answer,
            "stdout_contract": "caller_redirects_to_sink",
        },
        "checksums": {
            "route": route_checksum,
            "ranking": ranking_checksum,
            "frame": frame_checksum,
            "render": render_checksum,
        },
    });
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).expect("create output directory");
    }
    std::fs::write(&output, serde_json::to_vec_pretty(&report).unwrap())
        .expect("write performance report");
    if !thresholds_pass {
        std::process::exit(1);
    }
}
