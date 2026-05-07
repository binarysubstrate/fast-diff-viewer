//! Deterministic fixture generator for the bench harness.
//!
//! Regenerates `bench/2k_lines_a.txt`, `bench/2k_lines_b.txt`,
//! `bench/10k_lines_a.txt`, and `bench/10k_lines_b.txt` from a seeded
//! SplitMix64 PRNG. Same seed → byte-identical output, so the generator
//! and the committed fixtures stay in sync.
//!
//! Invoke: `cargo run --example gen_fixtures`
//!
//! This example duplicates `gen_lines` rather than depending on the
//! main crate's bench-gated module, so it builds without
//! `--features bench` and stays available for future regeneration.

use std::fs;
use std::io::Write;
use std::path::Path;

fn gen_lines(line_count: usize, seed: u64, divergence_every: usize) -> String {
    let mut rng_state = seed;
    let next = |s: &mut u64| -> u64 {
        *s = s.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = *s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    };
    let mut out = String::with_capacity(line_count * 48);
    for idx in 0..line_count {
        let tok_a = next(&mut rng_state) & 0xFFFF;
        let tok_b = next(&mut rng_state) & 0xFFFF;
        let tok_c = next(&mut rng_state) & 0xFFFF;
        let diverge = divergence_every > 0 && (idx % divergence_every == 0);
        if diverge {
            out.push_str(&format!(
                "line {idx:05}: MUT {tok_a:04x} {tok_b:04x} {tok_c:04x}\n"
            ));
        } else {
            out.push_str(&format!(
                "line {idx:05}: NOM {tok_a:04x} {tok_b:04x} {tok_c:04x}\n"
            ));
        }
    }
    out
}

fn write_file(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::File::create(path)?;
    f.write_all(body.as_bytes())?;
    Ok(())
}

fn main() {
    // `b` files differ from `a` by tagging every 20th line "MUT"
    // instead of "NOM" — exactly 5% divergence. Same seed otherwise.
    let seed_2k = 0x2BAD_F00D_DEAD_BEEFu64;
    let seed_10k = 0xC0DE_CAFE_2026_0419u64;

    let a2 = gen_lines(2000, seed_2k, 0);
    let b2 = gen_lines(2000, seed_2k, 20);
    let a10 = gen_lines(10000, seed_10k, 0);
    let b10 = gen_lines(10000, seed_10k, 20);

    write_file(Path::new("bench/2k_lines_a.txt"), &a2).expect("write 2k a");
    write_file(Path::new("bench/2k_lines_b.txt"), &b2).expect("write 2k b");
    write_file(Path::new("bench/10k_lines_a.txt"), &a10).expect("write 10k a");
    write_file(Path::new("bench/10k_lines_b.txt"), &b10).expect("write 10k b");

    println!("Fixtures written under bench/:");
    println!("  2k_lines_a.txt  ({} bytes)", a2.len());
    println!("  2k_lines_b.txt  ({} bytes)", b2.len());
    println!("  10k_lines_a.txt ({} bytes)", a10.len());
    println!("  10k_lines_b.txt ({} bytes)", b10.len());
}
