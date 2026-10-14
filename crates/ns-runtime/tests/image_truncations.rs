//! Every truncation of an image is refused with an error, never a panic:
//! an image is untrusted input (`docs/design/image-format.md`,
//! "Verification"). Three images -- a snapshot holding a closure and a
//! `ByteArray`, its packaged application, and a second packaged application
//! -- each cut at every length short of its own.
//!
//! One trial per image and per part of its lengths (`libtest-mimic`, so
//! `harness = false`): as one test, the loads of every length ran alone at
//! the end of the suite while the other cores idled. Each trial builds the
//! one image it cuts.

use std::path::PathBuf;
use std::rc::Rc;

use indoc::indoc;
use libtest_mimic::{Arguments, Trial};
use ns_runtime::image;
use ns_runtime::{
    ClassReference, HostResult, HostSignal, Interpreter, Mutator, Root, SourceOrigin, Value,
    ValueKind,
};

/// How many trials share one image's lengths.
const PARTS: usize = 8;

/// Builds one image.
type Build = fn() -> Vec<u8>;

/// The images, by name, each with the function that builds it.
const IMAGES: [(&str, Build); 3] = [
    ("snapshot", snapshot),
    ("packaged", packaged),
    ("packaged_application", packaged_application),
];

fn main() {
    let arguments = Arguments::from_args();
    let mut trials = Vec::new();
    for (image_name, build) in IMAGES {
        for part in 0..PARTS {
            trials.push(Trial::test(
                format!("every_truncation_is_refused::{image_name}::part_{part}_of_{PARTS}"),
                move || {
                    let bytes = build();
                    let length = bytes.len().div_ceil(PARTS);
                    let cuts = part * length..((part + 1) * length).min(bytes.len());
                    for cut in cuts {
                        // Never `{:?}` the `Ok` side: `LoadedImage` has no
                        // `Debug` impl at all (its `Interpreter` field has
                        // none either).
                        assert!(
                            image::load(&bytes[..cut]).is_err(),
                            "{image_name} cut at {cut} bytes loaded"
                        );
                    }
                    Ok(())
                },
            ));
        }
        trials.push(Trial::test(
            format!("the_untruncated_image_still_loads::{image_name}"),
            move || {
                if image::load(&build()).is_err() {
                    return Err(format!("{image_name} does not load").into());
                }
                Ok(())
            },
        ));
    }
    libtest_mimic::run(&arguments, trials).exit();
}

const SOURCE: &str = indoc! {"
    class X = Object new (
      | blk = [ 2 ]. |
    ) (
      public run = ( ^blk value )
    ) : ()
"};

/// A snapshot of an instance of `SOURCE`'s class, which holds a closure,
/// beside a `ByteArray`.
fn snapshot() -> Vec<u8> {
    let (mut interpreter, class) = load(SOURCE);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let origin = SourceOrigin::File(PathBuf::from("t.ns"));
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        let byte_array = mutator.new_byte_array(&byte_array_contents(13));
        let modules = [image::ModuleSource {
            class,
            origin: &origin,
            source: Some(SOURCE),
        }];
        image::snapshot(mutator, &modules, &[instance, Value::object(byte_array)])
            .expect("snapshot")
    })
}

/// The same instance packaged as an application.
fn packaged() -> Vec<u8> {
    let (mut interpreter, class) = load(SOURCE);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let origin = SourceOrigin::File(PathBuf::from("t.ns"));
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        let modules = [image::ModuleSource {
            class,
            origin: &origin,
            source: Some(SOURCE),
        }];
        image::package(mutator, &modules, instance).expect("package")
    })
}

fn packaged_application() -> Vec<u8> {
    let (mut interpreter, class) = load(indoc! {"
        class App = Object new (| public count ::= 41. |) (
            public main: platform args: arguments = ( count:: count + 1. ^count )
        ) : ()
    "});
    let application = expect_ok(interpreter.instantiate(&class, &[]), "App new");
    let origin = SourceOrigin::File("App.ns".into());
    interpreter
        .enter(|mutator| {
            let modules = [image::ModuleSource {
                class: class_reference(mutator, &class),
                origin: &origin,
                source: None,
            }];
            image::package(mutator, &modules, mutator.fetch(&application))
        })
        .unwrap_or_else(|error| panic!("{error}"))
}

fn load(source: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

/// `HostSignal` carries no `Debug` impl, so `Result::expect` is unusable on
/// a `HostResult` -- unwrap by hand.
fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// The class object `root` holds, fetched inside the entry that uses it.
fn class_reference<'gc>(mutator: &Mutator<'gc>, root: &Root) -> ClassReference<'gc> {
    match mutator.fetch(root).kind() {
        ValueKind::Object(class) => class,
        _ => panic!("expected a class object"),
    }
}

/// The bytes of a `ByteArray` of `size`: every value a byte takes, so that
/// a byte read from the wrong place shows.
fn byte_array_contents(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index * 7 + 3) as u8).collect()
}
