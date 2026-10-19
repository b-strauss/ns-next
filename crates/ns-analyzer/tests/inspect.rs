//! `ns-next-analyzer inspect` -- the structural listing of an artifact.

mod common;
use common::{build_image, golden, ns_next_analyzer, ScratchDirectory};

#[test]
fn inspect_reports_the_sections_symbols_and_mixins_of_an_image() {
    let directory = ScratchDirectory::new("arithmetic");
    let image = directory.join("app.nsi");
    std::fs::write(
        &image,
        build_image(
            &golden("library/number/integer/arithmetic"),
            ns_runtime::Lowering::Optimized,
            false,
        ),
    )
    .expect("write the image");

    let (ok, out, error) = ns_next_analyzer(&["inspect".as_ref(), image.as_os_str()]);
    assert!(ok, "{error}");

    assert!(out.contains("format version 1"), "{out}");
    assert!(!out.contains("stateless"), "{out}");
    assert!(out.contains("start: yes"), "{out}");
    assert!(out.contains("modules: 1"), "{out}");
    assert!(out.contains("source: no"), "{out}");
    for section in [
        "header",
        "symbols",
        "code",
        "mixins",
        "modules",
        "shipped names",
        "graph",
    ] {
        assert!(out.contains(section), "missing section `{section}`:\n{out}");
    }
    // The application's entry selector is in the symbol table because every
    // send instruction names its selector.
    assert!(out.contains("main:args:"), "{out}");
}

#[test]
fn inspect_reports_the_text_table_of_an_image_with_source() {
    let directory = ScratchDirectory::new("with-source");
    let image = directory.join("app.nsi");
    std::fs::write(
        &image,
        build_image(
            &golden("library/number/integer/arithmetic"),
            ns_runtime::Lowering::Optimized,
            true,
        ),
    )
    .expect("write the image");

    let (ok, out, error) = ns_next_analyzer(&["inspect".as_ref(), image.as_os_str()]);
    assert!(ok, "{error}");
    assert!(out.contains("source: yes"), "{out}");
    assert!(out.contains("texts ("), "{out}");
}

#[test]
fn inspect_reports_a_corrupt_artifact_rather_than_panicking() {
    let directory = ScratchDirectory::new("corrupt");
    let bad = directory.join("bad.img");
    let mut bytes = ns_runtime::image::encode::MAGIC.to_vec();
    bytes.extend_from_slice(&[1, 0, 0, 0]);
    std::fs::write(&bad, bytes).expect("write");

    let (ok, _, error) = ns_next_analyzer(&["inspect".as_ref(), bad.as_os_str()]);
    assert!(!ok);
    assert!(error.starts_with("error:"), "{error}");
    assert!(!error.contains("panicked"), "{error}");
}

/// The section line says how many bodies a code table holds; it did not say
/// which. That gap is what made the output hard to follow: a reader could see
/// that four bodies existed but not what to ask `disasm` for. The mixin table
/// already maps selector to code id, so naming them costs no new decoding.
#[test]
fn inspect_names_every_body_in_the_code_table() {
    let directory = ScratchDirectory::new("code-table");
    let out = directory.join("app.nsi");
    std::fs::write(
        &out,
        build_image(
            &golden("library/number/integer/arithmetic"),
            ns_runtime::Lowering::Optimized,
            false,
        ),
    )
    .expect("write the image");

    let (ok, listing, error) = ns_next_analyzer(&["inspect".as_ref(), out.as_os_str()]);
    assert!(ok, "{error}");

    assert!(listing.contains("code table"), "{listing}");
    // arithmetic.ns declares one method and one slot initializer on a root
    // class whose superclass clause is `Object new`.
    assert!(listing.contains("main:args:"), "{listing}");
    assert!(listing.contains("initializer"), "{listing}");
    assert!(listing.contains("superclass clause"), "{listing}");
    // Every body must be accounted for -- an unnamed row would be exactly the
    // gap this closes.
    assert!(
        !listing.contains("  (unnamed)"),
        "every code id should be attributed:\n{listing}"
    );
}

/// `--full` is the `javap -v` of this format: everything the artifact says,
/// rendered, so a person can follow it end to end from the text alone. The
/// default stays a table of contents, because a real module's full dump runs
/// to thousands of lines.
#[test]
fn inspect_full_renders_every_mixin_and_every_body() {
    let directory = ScratchDirectory::new("full");
    let out = directory.join("app.nsi");
    std::fs::write(
        &out,
        build_image(
            &golden("library/number/integer/arithmetic"),
            ns_runtime::Lowering::Optimized,
            false,
        ),
    )
    .expect("write the image");

    let (ok, brief, error) = ns_next_analyzer(&["inspect".as_ref(), out.as_os_str()]);
    assert!(ok, "{error}");
    let (ok, full, error) =
        ns_next_analyzer(&["inspect".as_ref(), out.as_os_str(), "--full".as_ref()]);
    assert!(ok, "{error}");

    // The default is unchanged by the flag's existence.
    assert!(brief.contains("code table"), "{brief}");
    assert!(
        !brief.contains("SendPublic"),
        "the default must not disassemble:\n{brief}"
    );

    // Expanded mixin rows: the slot's name, not just how many there are.
    assert!(full.contains("result"), "{full}");
    assert!(full.contains("new/0"), "{full}");
    assert!(full.contains("main:args: -> code"), "{full}");

    // Every body, disassembled, labelled with the id the code table names.
    assert!(full.contains("bodies"), "{full}");
    assert!(full.contains("SendPublic"), "{full}");
    assert!(full.contains("InitSlot"), "{full}");
    assert!(
        full.len() > brief.len() * 2,
        "full should be substantially longer"
    );

    // `main:args:` sends four times in one family (`p transcript`, `result
    // printString`, the `show:` and `newLine` cascade sends), so a real
    // per-family numbering shows more than one distinct call site. Without
    // `inspect`'s own call to `number_call_site_families`, every send here
    // would decode with the same `@0` placeholder instead -- silently, since
    // nothing else in this dump depends on the numbers being real.
    assert!(
        full.contains("@1"),
        "a family with more than one send must show more than one call-site number:\n{full}"
    );
}
