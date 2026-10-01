use indoc::indoc;

#[test]
fn hello_world_ast() {
    let source = indoc! {"
        class Hello usingPlatform: platform = (
            | private Transcript = platform Transcript. |
        ) (
            public main: args = ( Transcript show: 'Hello, World!' )
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    insta::assert_debug_snapshot!(module);
}
