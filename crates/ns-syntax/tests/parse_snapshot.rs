use indoc::indoc;

#[test]
fn hello_world_ast() {
    let src = indoc! {"
        Newspeak3
        'HelloApp'
        class Hello usingPlatform: platform = (
            | private Transcript = platform Transcript. |
        ) (
            public main: args = ( Transcript show: 'Hello, World!' )
        ) : ()
    "};
    let module = ns_syntax::parse(src).expect("parse");
    insta::assert_debug_snapshot!(module);
}
