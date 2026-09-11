//! Isolated, in-memory storage/CPU experiment; not a production storage reader.
use fastcdc::v2020::FastCDC;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    time::Instant,
};

type Id = [u8; 32];
type Objects = HashMap<Id, (usize, Vec<u8>)>;
const PASSES: usize = 5;

#[derive(Deserialize)]
struct Corpus {
    head: String,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    versions: Vec<Version>,
}
#[derive(Deserialize)]
struct Version {
    file: String,
    size: usize,
    sha: String,
}
#[derive(Clone)]
struct Reference {
    id: Id,
    len: u32,
}
#[derive(Default, Serialize)]
struct Phases {
    full_sha_us: f64,
    boundaries_us: f64,
    chunk_sha_us: f64,
    lookup_compress_missing_us: f64,
}
struct Encoded {
    recipes: HashMap<Id, Vec<u8>>,
    objects: Objects,
    versions: Vec<Id>,
    latencies_us: Vec<f64>,
    total_us: f64,
    phases: Phases,
}
#[derive(Serialize)]
struct Stats {
    mean_us: f64,
    p50_us: f64,
    p95_us: f64,
    max_us: f64,
}
#[derive(Serialize)]
struct Storage {
    versions: usize,
    unique_files: usize,
    objects: usize,
    raw_object_bytes: usize,
    payload_bytes: usize,
    recipe_bytes: usize,
}
#[derive(Serialize)]
struct Row {
    method: String,
    target: usize,
    checkpoint: Stats,
    cold_first_checkpoint: Stats,
    history_pass: Stats,
    throughput_mib_s: f64,
    diagnostic_phase_mean: Phases,
    full: Storage,
    thinned: Storage,
    verification_us_per_version: f64,
    verified_full: usize,
    verified_thinned: usize,
}
#[derive(Serialize)]
struct CaseReport {
    name: String,
    versions: usize,
    source_bytes: usize,
    rows: Vec<Row>,
}
#[derive(Serialize)]
struct Report {
    source_commit: String,
    cpu: String,
    hardware_threads: usize,
    rustc: String,
    zstd_runtime: String,
    fastcdc: String,
    sha2: String,
    warmups: usize,
    timed_passes: usize,
    timing: String,
    recipe: String,
    cases: Vec<CaseReport>,
}

fn sha(bytes: &[u8]) -> Id {
    Sha256::digest(bytes).into()
}
fn elapsed(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e6
}
fn diagnostic_start(on: bool) -> Option<Instant> {
    on.then(Instant::now)
}
fn diagnostic_elapsed(t: Option<Instant>) -> f64 {
    t.map_or(0.0, elapsed)
}

// Full SHA remains the encoding lookup key. Every reference uses a RAW SHA-256.
// Magic's final byte distinguishes whole-file (0) from CDC (1).
fn recipe(id: Id, len: usize, refs: &[Reference], target: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(48 + 36 * refs.len());
    bytes.extend_from_slice(if target == 0 { b"HR1\0" } else { b"HR1\x01" });
    bytes.extend_from_slice(&id);
    bytes.extend_from_slice(&(len as u64).to_le_bytes());
    bytes.extend_from_slice(
        &u32::try_from(refs.len())
            .expect("bounded ref count")
            .to_le_bytes(),
    );
    for r in refs {
        bytes.extend_from_slice(&r.id);
        bytes.extend_from_slice(&r.len.to_le_bytes());
    }
    bytes
}

fn parse(bytes: &[u8]) -> Result<(Id, usize, Vec<Reference>), String> {
    if bytes.len() < 48 || (&bytes[..4] != b"HR1\0" && &bytes[..4] != b"HR1\x01") {
        return Err("invalid recipe header".into());
    }
    let id = bytes[4..36].try_into().expect("fixed header");
    let len = usize::try_from(u64::from_le_bytes(
        bytes[36..44].try_into().expect("fixed header"),
    ))
    .map_err(|_| "file length overflow")?;
    let count = u32::from_le_bytes(bytes[44..48].try_into().expect("fixed header")) as usize;
    if count.checked_mul(36).and_then(|n| n.checked_add(48)) != Some(bytes.len()) {
        return Err("invalid recipe length".into());
    }
    let refs = bytes[48..]
        .as_chunks::<36>()
        .0
        .iter()
        .map(|r| Reference {
            id: r[..32].try_into().expect("fixed entry"),
            len: u32::from_le_bytes(r[32..].try_into().expect("fixed entry")),
        })
        .collect();
    Ok((id, len, refs))
}

fn encode(files: &[Vec<u8>], target: usize, diagnostic: bool) -> Encoded {
    let history_start = Instant::now();
    let mut compressor = zstd::bulk::Compressor::new(3).expect("zstd context");
    let mut out = Encoded {
        recipes: HashMap::new(),
        objects: HashMap::new(),
        versions: Vec::new(),
        latencies_us: Vec::new(),
        total_us: 0.0,
        phases: Phases::default(),
    };
    for data in files {
        let start = Instant::now();
        let stage = diagnostic_start(diagnostic);
        let id = sha(data);
        out.phases.full_sha_us += diagnostic_elapsed(stage);
        if !out.recipes.contains_key(&id) {
            let stage = diagnostic_start(diagnostic);
            let ranges: Vec<(usize, usize)> = if target == 0 {
                vec![(0, data.len())]
            } else {
                FastCDC::new(data, target / 4, target, target * 8)
                    .map(|c| (c.offset, c.length))
                    .collect()
            };
            out.phases.boundaries_us += diagnostic_elapsed(stage);
            let stage = diagnostic_start(diagnostic);
            let refs: Vec<Reference> = ranges
                .iter()
                .map(|&(offset, len)| Reference {
                    id: if target == 0 {
                        id
                    } else {
                        sha(&data[offset..offset + len])
                    },
                    len: u32::try_from(len).expect("bounded chunk size"),
                })
                .collect();
            out.phases.chunk_sha_us += diagnostic_elapsed(stage);
            let stage = diagnostic_start(diagnostic);
            for (r, &(offset, len)) in refs.iter().zip(&ranges) {
                out.objects.entry(r.id).or_insert_with(|| {
                    (
                        len,
                        compressor
                            .compress(&data[offset..offset + len])
                            .expect("compress chunk"),
                    )
                });
            }
            out.phases.lookup_compress_missing_us += diagnostic_elapsed(stage);
            out.recipes
                .insert(id, recipe(id, data.len(), &refs, target));
        }
        out.versions.push(id);
        out.latencies_us.push(elapsed(start));
    }
    out.total_us = elapsed(history_start);
    out
}

fn reconstruct(bytes: &[u8], objects: &Objects) -> Result<Vec<u8>, String> {
    let (id, len, refs) = parse(bytes)?;
    let mut out = Vec::new();
    for r in refs {
        let (raw_len, compressed) = objects.get(&r.id).ok_or("missing chunk")?;
        if *raw_len != r.len as usize {
            return Err("wrong chunk length".into());
        }
        let raw = zstd::bulk::decompress(compressed, *raw_len).map_err(|e| e.to_string())?;
        if raw.len() != *raw_len || sha(&raw) != r.id {
            return Err("corrupt chunk".into());
        }
        out.extend_from_slice(&raw);
    }
    if out.len() != len || sha(&out) != id {
        return Err("corrupt file".into());
    }
    Ok(out)
}

fn reachable(encoded: &Encoded, indices: &[usize]) -> HashSet<Id> {
    indices
        .iter()
        .flat_map(|&i| {
            parse(&encoded.recipes[&encoded.versions[i]])
                .expect("own recipe")
                .2
                .into_iter()
                .map(|r| r.id)
        })
        .collect()
}
fn storage(encoded: &Encoded, indices: &[usize]) -> Storage {
    let keys = reachable(encoded, indices);
    let files: HashSet<_> = indices.iter().map(|&i| encoded.versions[i]).collect();
    Storage {
        versions: indices.len(),
        unique_files: files.len(),
        objects: keys.len(),
        raw_object_bytes: keys.iter().map(|id| encoded.objects[id].0).sum(),
        payload_bytes: keys.iter().map(|id| encoded.objects[id].1.len()).sum(),
        recipe_bytes: files.iter().map(|id| encoded.recipes[id].len()).sum(),
    }
}
fn verify(encoded: &Encoded, files: &[Vec<u8>], indices: &[usize], objects: &Objects) {
    for &i in indices {
        let id = encoded.versions[i];
        let decoded = reconstruct(&encoded.recipes[&id], objects).expect("verified reconstruction");
        assert_eq!(sha(&decoded), id);
        assert_eq!(decoded, files[i]);
    }
}
fn stats(samples: &[f64]) -> Stats {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| sorted[((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    Stats {
        mean_us: samples.iter().sum::<f64>() / samples.len() as f64,
        p50_us: percentile(0.5),
        p95_us: percentile(0.95),
        max_us: percentile(1.0),
    }
}

fn measure(files: &[Vec<u8>], target: usize) -> Row {
    // Warmup and diagnostic pass are separate from the five timing passes.
    drop(encode(files, target, false));
    let mut checkpoints = Vec::new();
    let mut histories = Vec::new();
    let mut cold = Vec::new();
    for _ in 0..PASSES {
        let run = encode(files, target, false);
        histories.push(run.total_us);
        cold.push(run.latencies_us[0]);
        checkpoints.extend_from_slice(&run.latencies_us);
        // Store/map destruction is outside service-time measurements.
    }
    let mut encoded = encode(files, target, true);
    let all: Vec<_> = (0..files.len()).collect();
    let thin: Vec<_> = (0..files.len())
        .filter(|&i| i == 0 || i % 20 == 0 || i + 1 == files.len())
        .collect();
    let verification_start = Instant::now();
    verify(&encoded, files, &all, &encoded.objects);
    let verification_us = elapsed(verification_start);
    // Physically exclude unreachable objects in memory, then decode retained versions.
    let keep = reachable(&encoded, &thin);
    let thin_objects: Objects = encoded
        .objects
        .iter()
        .filter(|(id, _)| keep.contains(*id))
        .map(|(id, value)| (*id, value.clone()))
        .collect();
    verify(&encoded, files, &thin, &thin_objects);
    for phase in [
        &mut encoded.phases.full_sha_us,
        &mut encoded.phases.boundaries_us,
        &mut encoded.phases.chunk_sha_us,
        &mut encoded.phases.lookup_compress_missing_us,
    ] {
        *phase /= files.len() as f64;
    }
    let history_pass = stats(&histories);
    Row {
        method: if target == 0 {
            "whole-zstd3".into()
        } else {
            format!("fastcdc-{target}-zstd3")
        },
        target,
        checkpoint: stats(&checkpoints),
        cold_first_checkpoint: stats(&cold),
        throughput_mib_s: files.iter().map(Vec::len).sum::<usize>() as f64
            / (1024.0 * 1024.0)
            / (history_pass.mean_us / 1e6),
        history_pass,
        full: storage(&encoded, &all),
        thinned: storage(&encoded, &thin),
        diagnostic_phase_mean: encoded.phases,
        verification_us_per_version: verification_us / files.len() as f64,
        verified_full: all.len(),
        verified_thinned: thin.len(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 3 {
        return Err("Usage: native-fastcdc-bench CORPUS_JSON RESULTS_JSON".into());
    }
    let corpus: Corpus = serde_json::from_slice(&fs::read(&args[1])?)?;
    let mut cases = Vec::new();
    for case in corpus.cases {
        let files: Vec<_> = case
            .versions
            .iter()
            .map(|v| {
                let data = fs::read(&v.file).expect("corpus file");
                assert_eq!(data.len(), v.size);
                assert_eq!(format!("{:x}", Sha256::digest(&data)), v.sha);
                data
            })
            .collect();
        assert!(!files.is_empty());
        cases.push(CaseReport {
            name: case.name,
            versions: files.len(),
            source_bytes: files.iter().map(Vec::len).sum(),
            rows: [0, 1024, 4096]
                .into_iter()
                .map(|target| measure(&files, target))
                .collect(),
        });
    }
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu = cpuinfo
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map_or("unknown", |(_, v)| v.trim())
        .to_owned();
    let rustc = std::process::Command::new("rustc")
        .arg("--version")
        .output()?;
    let report = Report {
        source_commit: corpus.head, cpu,
        hardware_threads: std::thread::available_parallelism()?.get(),
        rustc: String::from_utf8(rustc.stdout)?.trim().to_owned(),
        zstd_runtime: zstd::zstd_safe::version_string().into(),
        fastcdc: "5.0.0 v2020, normalization Level1, seed 0, min=target/4 max=target*8".into(),
        sha2: "0.10 (exact version in Cargo.lock)".into(),
        warmups: 1, timed_passes: PASSES,
        timing: "microseconds; fresh document-local stores each pass; uninstrumented main pass, separate diagnostic stages; no disk IO; destruction excluded".into(),
        recipe: "48-byte header (magic4/fullSHA32/fileLen8/count4), each ordered ref SHA32/rawLen4; actual serialization; raw object IDs; common tree/event records and storage indexes excluded".into(),
        cases,
    };
    fs::write(&args[2], serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixtures() -> Vec<Vec<u8>> {
        let source: Vec<u8> = (0..20000)
            .map(|i| ((i * 31 + i / 17) % 251) as u8)
            .collect();
        let mut edited = b"an insertion".to_vec();
        edited.extend_from_slice(&source);
        vec![
            Vec::new(),
            b"hello".to_vec(),
            source.clone(),
            edited,
            source,
        ]
    }
    #[test]
    fn roundtrip_empty_small_large_and_duplicates() {
        let files = fixtures();
        for target in [0, 1024, 4096] {
            let encoded = encode(&files, target, false);
            verify(&encoded, &files, &[0, 1, 2, 3, 4], &encoded.objects);
            assert_eq!(encoded.recipes.len(), 4);
            assert_eq!(storage(&encoded, &[2, 4]).unique_files, 1);
            for bytes in encoded.recipes.values() {
                assert_eq!(bytes.len(), 48 + 36 * parse(bytes).expect("recipe").2.len());
            }
        }
    }
    #[test]
    fn missing_corrupt_and_reordered_chunks_fail() {
        let files = fixtures();
        let mut encoded = encode(&files, 1024, false);
        let id = encoded.versions[2];
        let bytes = &encoded.recipes[&id];
        let refs = parse(bytes).expect("recipe").2;
        assert!(refs.len() > 1);
        let removed = encoded.objects.remove(&refs[0].id).expect("object");
        assert!(reconstruct(bytes, &encoded.objects).is_err());
        encoded
            .objects
            .insert(refs[0].id, (removed.0, vec![0, 1, 2]));
        assert!(reconstruct(bytes, &encoded.objects).is_err());
        encoded.objects.insert(refs[0].id, removed);
        let mut reversed = refs;
        reversed.reverse();
        assert!(reconstruct(
            &recipe(id, files[2].len(), &reversed, 1024),
            &encoded.objects
        )
        .is_err());
        assert!(parse(&bytes[..47]).is_err());
    }
    #[test]
    fn retention_excludes_unreachable_objects() {
        let files = fixtures();
        let mut encoded = encode(&files, 1024, false);
        let before = storage(&encoded, &[0, 1, 2, 3, 4]);
        let keep = reachable(&encoded, &[1]);
        encoded.objects.retain(|id, _| keep.contains(id));
        verify(&encoded, &files, &[1], &encoded.objects);
        assert!(storage(&encoded, &[1]).payload_bytes < before.payload_bytes);
    }
    #[test]
    fn quantiles_are_checkpoint_samples_not_cumulative_times() {
        let s = stats(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(s.mean_us, 3.0);
        assert_eq!(s.p50_us, 3.0);
        assert_eq!(s.p95_us, 5.0);
    }
}
