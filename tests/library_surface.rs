#[test]
fn library_surface_is_intentional() {
    let t = trybuild::TestCases::new();
    t.pass("tests/fixtures/library_surface/public_api.rs");
    t.compile_fail("tests/fixtures/library_surface/anki_*.rs");
    t.compile_fail("tests/fixtures/library_surface/generate_*.rs");
    t.compile_fail("tests/fixtures/library_surface/root_*.rs");
}
