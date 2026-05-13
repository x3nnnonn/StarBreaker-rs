use starbreaker_datacore::Database;
use std::{env, fs, time::Instant};

fn main() {
    let path = env::args()
        .nth(1)
        .expect("Usage: bench_parallel <path.dcb>");
    let data = fs::read(&path).expect("failed to read");

    let db = Database::from_bytes(&data).expect("failed to parse");

    bench_format(
        "JSON",
        &db,
        |db, r, buf| starbreaker_datacore::export::write_json(db, r, buf).map(|_| ()),
        |db, r| starbreaker_datacore::export::to_json(db, r).map(|b| b.len()),
    );

    bench_format(
        "XML (sb)",
        &db,
        |db, r, buf| starbreaker_datacore::export::write_xml(db, r, buf).map(|_| ()),
        |db, r| starbreaker_datacore::export::to_xml(db, r).map(|b| b.len()),
    );

    bench_format(
        "XML (unp4k)",
        &db,
        |db, r, buf| {
            starbreaker_datacore::export::to_unp4k_xml(db, r).map(|b| {
                use std::io::Write;
                buf.write_all(&b).unwrap();
            })
        },
        |db, r| starbreaker_datacore::export::to_unp4k_xml(db, r).map(|b| b.len()),
    );

    bench_format(
        "XML (DataForge)",
        &db,
        |db, r, buf| starbreaker_datacore::export::write_dataforge_xml(db, r, buf).map(|_| ()),
        |db, r| starbreaker_datacore::export::to_dataforge_xml(db, r).map(|b| b.len()),
    );
}

fn bench_format<S, A>(label: &str, db: &Database, stream: S, alloc: A)
where
    S: Fn(&Database, &starbreaker_datacore::types::Record, &mut Vec<u8>)
        -> Result<(), starbreaker_datacore::error::ExportError>,
    A: Fn(&Database, &starbreaker_datacore::types::Record)
        -> Result<usize, starbreaker_datacore::error::ExportError>
        + Sync,
{
    println!("=== {label} ===");

    let start = Instant::now();
    let mut total_bytes_st = 0usize;
    let mut count_st = 0u32;
    for record in db.records() {
        if !db.is_main_record(record) {
            continue;
        }
        if let Ok(n) = alloc(db, record) {
            total_bytes_st += n;
            count_st += 1;
        }
    }
    let st_time = start.elapsed();
    println!(
        "  Single-threaded (alloc):  {:?} ({count_st} records, {:.1} MB)",
        st_time,
        total_bytes_st as f64 / 1_048_576.0
    );

    let start = Instant::now();
    let mut total_bytes_reuse = 0usize;
    let mut count_reuse = 0u32;
    let mut buf: Vec<u8> = Vec::with_capacity(256 * 1024);
    for record in db.records() {
        if !db.is_main_record(record) {
            continue;
        }
        buf.clear();
        if stream(db, record, &mut buf).is_ok() {
            total_bytes_reuse += buf.len();
            count_reuse += 1;
        }
    }
    let reuse_time = start.elapsed();
    println!(
        "  Single-threaded (reuse):  {:?} ({count_reuse} records, {:.1} MB)",
        reuse_time,
        total_bytes_reuse as f64 / 1_048_576.0
    );

    {
        use rayon::prelude::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let start = Instant::now();
        let total_bytes = AtomicUsize::new(0);
        let count = AtomicUsize::new(0);
        db.records().par_iter().for_each(|record| {
            if !db.is_main_record(record) {
                return;
            }
            if let Ok(n) = alloc(db, record) {
                total_bytes.fetch_add(n, Ordering::Relaxed);
                count.fetch_add(1, Ordering::Relaxed);
            }
        });
        let par_time = start.elapsed();
        let c = count.load(Ordering::Relaxed);
        let b = total_bytes.load(Ordering::Relaxed);
        println!(
            "  Parallel (rayon):         {:?} ({c} records, {:.1} MB)",
            par_time,
            b as f64 / 1_048_576.0
        );
        println!(
            "  Speedup vs alloc: {:.1}x",
            st_time.as_secs_f64() / par_time.as_secs_f64()
        );
    }
    println!();
}
