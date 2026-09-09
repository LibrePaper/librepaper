use librepaper::session;
use std::time::Instant;

fn measure(label: &str, files: Vec<(&str, String)>) {
    let doc = session::new_doc();
    let mut main_id = String::new();
    for (path, body) in &files {
        let id = session::put_text(&doc, path, body);
        if main_id.is_empty() {
            main_id = id;
        }
    }
    session::set_main(&doc, &main_id);
    assert_eq!(session::paths_of(&doc).len(), files.len());
    let t0 = Instant::now();
    let state = session::encode_state(&doc);
    let encode_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let texts = session::texts_of(&doc);
    let text_bytes: usize = texts.values().map(String::len).sum();
    let paths_bytes: usize = texts.keys().map(String::len).sum();
    println!("{label}\tfiles={}\ttext_bytes={text_bytes}\tpath_bytes={paths_bytes}\tyjs_state_bytes={}\tencode_ms={encode_ms:.3}", files.len(), state.len());
    let t1 = Instant::now();
    let _ = session::encode_vector(&doc);
    let vector_ms = t1.elapsed().as_secs_f64() * 1000.0;
    println!("{label}\tstate_vector_ms={vector_ms:.3}");
}

fn measure_edited(label: &str, size: usize) {
    let doc = session::new_doc();
    let mut body = "x".repeat(size);
    let id = session::put_text(&doc, "main.md", &body);
    session::set_main(&doc, &id);
    assert_eq!(session::paths_of(&doc).len(), 1);
    for i in 0..10 {
        body.replace_range(i % size..(i % size) + 1, if i % 2 == 0 { "y" } else { "z" });
        session::replace_text(&doc, &body, "main.md");
        assert_eq!(session::paths_of(&doc).len(), 1);
        assert_eq!(session::text_of(&doc), body);
    }
    for _ in 0..3 {
        std::hint::black_box(session::encode_state(&doc));
    }
    let mut times = Vec::new();
    let mut state_len = 0;
    for _ in 0..20 {
        let t = Instant::now();
        state_len = session::encode_state(&doc).len();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("{label}\tfinal_text_bytes={size}\tyjs_state_bytes={state_len}\tencode_ms_median={:.3}\tp95={:.3}", (times[9] + times[10]) / 2.0, times[18]);
}

fn retain_many(label: &str, n: usize, body: &str) {
    let mut docs = Vec::with_capacity(n);
    let start = Instant::now();
    for i in 0..n {
        let doc = session::new_doc();
        let id = session::put_text(&doc, "main.md", &format!("{body}\n# document {i}"));
        session::set_main(&doc, &id);
        assert_eq!(session::paths_of(&doc).len(), 1);
        docs.push(doc);
    }
    let construct_s = start.elapsed().as_secs_f64();
    let encode_start = Instant::now();
    let total_state: usize = docs
        .iter()
        .map(|doc| session::encode_state(doc).len())
        .sum();
    let encode_s = encode_start.elapsed().as_secs_f64();
    println!("{label}\tdocs={n}\ttotal_encoded_state_bytes={total_state}\tconstruct_s={construct_s:.3}\tencode_s={encode_s:.3}");
    std::hint::black_box(docs);
}

fn compare_delta(label: &str, initial: &str, wanted: &str) {
    let doc = session::new_doc();
    assert!(!doc.skip_gc(), "The measurement assumes normal Yrs GC");
    let id = session::put_text(&doc, "main.md", initial);
    session::set_main(&doc, &id);
    let base = session::encode_state(&doc);
    let vector = session::encode_vector(&doc);
    session::replace_text(&doc, wanted, "main.md");
    assert_eq!(session::paths_of(&doc).len(), 1);
    assert_eq!(session::text_of(&doc), wanted);
    let full = session::encode_state(&doc);
    let delta = session::encode_diff(&doc, &vector).unwrap();

    // Validate the actual recovery sequence, not just the full-state encoding.
    let recovered = session::new_doc();
    session::apply_update(&recovered, &base).unwrap();
    assert_eq!(session::text_of(&recovered), initial);
    session::apply_update(&recovered, &delta).unwrap();
    assert_eq!(session::text_of(&recovered), wanted);
    session::apply_update(&recovered, &delta).unwrap();
    assert_eq!(session::text_of(&recovered), wanted);
    assert_eq!(session::paths_of(&recovered).len(), 1);
    println!(
        "{label}\tfull_state_bytes={}\tdelta_bytes={}\tstate_vector_changed={}",
        full.len(),
        delta.len(),
        vector != session::encode_vector(&doc)
    );
}

fn main() {
    measure(
        "small",
        vec![("main.md", "# Hello\n\nA short document.\n".to_string())],
    );
    measure("multi", vec![
        ("main.tex", "\\documentclass{article}\n\\begin{document}\nHello\n\\input{chapters/one.tex}\n\\end{document}\n".to_string()),
        ("chapters/one.tex", "A chapter with a little text.\n".repeat(100)),
        ("refs.bib", "@article{a, title={Example}, author={Doe}}\n".repeat(50)),
        ("fig/plot.svg", "<svg></svg>\n".repeat(100)),
    ]);
    measure(
        "oversized_unadmitted_5_7m",
        vec![(
            "main.md",
            "Lorem ipsum dolor sit amet, consectetur adipiscing elit.\n".repeat(100_000),
        )],
    );
    let mut files = Vec::new();
    for i in 0..200 {
        files.push((
            Box::leak(format!("chapters/{i:03}.md").into_boxed_str()) as &str,
            "x\n".repeat(100),
        ));
    }
    measure("max_files_text_only", files);
    measure_edited("edited_10k", 10_000);
    measure_edited("edited_100k", 100_000);
    measure_edited("edited_1m", 1_000_000);
    let initial = "x".repeat(10_000);
    let appended = format!("{initial}Y");
    compare_delta("delta_insert_10k", &initial, &appended);
    compare_delta("delta_delete_10k", &appended, &initial);
    compare_delta("delta_replace_10k", &initial, &"y".repeat(10_000));
    retain_many("1000_small_live", 1000, "# Hello\nA short document.");
    retain_many(
        "1000_4k_live",
        1000,
        &"Lorem ipsum dolor sit amet, consectetur adipiscing elit.\n".repeat(70),
    );
    retain_many(
        "1000_100k_live",
        1000,
        &"Lorem ipsum dolor sit amet, consectetur adipiscing elit.\n".repeat(1750),
    );
}
