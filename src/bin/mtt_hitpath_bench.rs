//! Isolated hit-path microbenchmark — the exp19 registration gate.
//!
//! Measures, per call, the two candidate read disciplines for a POSITIVE
//! index-mode accessor (the post-exp18 residual: ~785M positive
//! `inner_ref_index` calls/run on PLN Robot):
//!
//!   A. the LIVE shadow hit path — `as_atom()` on a warm `INNER_SHADOW`
//!      (TLS + RefCell borrow + epoch check + two-level page chase);
//!   B. a MOCKED arena-coresident Inner column — one prebuilt
//!      `Vec<MettaValueInner>` indexed by `Addr::raw()`, read with two
//!      loads + the same variant match (NO TLS, NO RefCell, NO epoch,
//!      NO lock — the launder discipline the real column would use).
//!
//! Decision rule (docs/cesk-gc/f1-profile-post309-2026-06-11.md, R1-F6):
//! projected Robot delta = (ns_A − ns_B) × 785e6; if that is < ~3% of the
//! ~10s Robot wall (< ~0.3s), the Inner-column lever pivots to
//! call-volume reduction instead. Both prior projection methods (Ir
//! share, cycle counts) each missed badly once — this measures.
//!
//! Run (index build only):
//!   cargo build --release --features index-gc --bin mtt-hitpath-bench
//!   taskset -c 8 ./target/release/mtt-hitpath-bench
#[cfg(feature = "index-gc")]
fn main() {
    use mettatron::backend::models::{MettaValue, MettaValueFactory, MettaValueInner};
    use std::hint::black_box;
    use std::time::Instant;

    // The index build starts in index mode (GC_MODE=1) — no flip needed.
    let f = mettatron::backend::models::global_factory();

    // ~1M mixed nodes: 50% atoms / 50% small ground sexprs — roughly the
    // accessor-positive mix the profile shows (as_atom + as_sexpr heavy).
    const N: usize = 1_000_000;
    const PASSES: usize = 40; // ~40M calls per arm
    let mut handles: Vec<MettaValue> = Vec::with_capacity(N);
    let mut names: Vec<String> = Vec::with_capacity(1000);
    for i in 0..1000 {
        names.push(format!("atom-{i}"));
    }
    for i in 0..N {
        if i % 2 == 0 {
            handles.push(f.atom(&names[i % 1000]));
        } else {
            let a = f.atom(&names[(i + 7) % 1000]);
            let b = f.long((i % 4096) as i64);
            handles.push(f.sexpr_from_slice(&[a, b]));
        }
    }

    // ── Arm A: live shadow hit path (warm it with one full pass first) ──
    let mut acc: u64 = 0;
    for h in &handles {
        if let Some(s) = h.as_atom() {
            acc = acc.wrapping_add(s.len() as u64);
        }
    }
    let t0 = Instant::now();
    for _ in 0..PASSES {
        for h in &handles {
            if let Some(s) = black_box(h).as_atom() {
                acc = acc.wrapping_add(s.as_ptr() as u64 & 0xFF);
            }
        }
    }
    let a_ns = t0.elapsed().as_nanos() as f64 / (PASSES * N) as f64;

    // ── Arm B: mocked Inner column — prebuilt, dense-indexed, two loads ──
    // (The real column lives per-segment beside the nodes; a dense Vec is
    // the same read shape: base + offset load, then the variant match.)
    let max_raw = handles
        .iter()
        .filter_map(|h| h.as_arena_addr_raw_for_bench())
        .max()
        .expect("heap handles exist") as usize;
    let mut column: Vec<Option<MettaValueInner>> = Vec::with_capacity(max_raw + 1);
    column.resize_with(max_raw + 1, || None);
    for h in &handles {
        if let Some(raw) = h.as_arena_addr_raw_for_bench() {
            column[raw as usize] = Some(h.materialize_inner_for_bench());
        }
    }
    let col = column.as_slice();
    let t1 = Instant::now();
    for _ in 0..PASSES {
        for h in &handles {
            if let Some(raw) = black_box(h).as_arena_addr_raw_for_bench() {
                // Two loads (slice base is register-resident; entry load) +
                // the same Atom/Spanned match the accessor performs.
                if let Some(MettaValueInner::Atom(s)) =
                    unsafe { col.get_unchecked(raw as usize) }.as_ref()
                {
                    acc = acc.wrapping_add(s.as_ptr() as u64 & 0xFF);
                }
            }
        }
    }
    let b_ns = t1.elapsed().as_nanos() as f64 / (PASSES * N) as f64;

    let delta_ns = a_ns - b_ns;
    let robot_calls = 785e6_f64;
    let projected_s = delta_ns * robot_calls / 1e9;
    println!("hit-path A (live shadow): {a_ns:.2} ns/call");
    println!("hit-path B (column mock): {b_ns:.2} ns/call");
    println!("delta: {delta_ns:.2} ns/call");
    println!(
        "projected Robot delta @785M calls: {projected_s:.3} s ({:.1}% of ~10s)",
        projected_s * 10.0
    );
    println!(
        "GATE: {}",
        if projected_s >= 0.3 {
            "PROCEED — register exp19 on the Inner column"
        } else {
            "PIVOT — call-volume reduction instead (column win under the 3% bar)"
        }
    );
    let _ = black_box(acc);
}

#[cfg(not(feature = "index-gc"))]
fn main() {
    eprintln!("mtt-hitpath-bench requires --features index-gc");
    std::process::exit(2);
}
