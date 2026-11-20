//! JSON decoding and encoding, for the agent protocol and other programs
//! that speak it.

mod common;

use common::Harness;

fn h() -> Harness {
    Harness::new(60, 6)
}

#[test]
fn it_decodes_the_shapes_of_a_real_message() {
    let mut h = h();
    h.eval(
        r#"(define d (json-decode "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"sessionId\":\"abc\",\"n\":42,\"pi\":3.5,\"ok\":true,\"no\":false,\"nil\":null,\"list\":[1,\"two\",[3]]}}"))
           (define r (json-ref d 'result))"#,
    );
    assert_eq!(h.eval("(json-ref d 'jsonrpc)"), "\"2.0\"");
    assert_eq!(h.eval("(json-ref d 'id)"), "1");
    assert_eq!(h.eval("(json-ref r 'sessionId)"), "\"abc\"");
    assert_eq!(h.eval("(json-ref r 'n)"), "42");
    assert_eq!(h.eval("(json-ref r 'pi)"), "3.5");
    assert_eq!(
        h.eval("(list (json-ref r 'ok) (json-ref r 'no) (json-ref r 'nil))"),
        "(#t #f null)"
    );
    // Arrays are vectors; nested arrays too.
    assert_eq!(
        h.eval("(vector->list (json-ref r 'list))"),
        "(1 \"two\" #(3))"
    );
    // A missing key is #f, or the given default.
    assert_eq!(h.eval("(json-ref r 'missing)"), "#f");
    assert_eq!(h.eval("(json-ref r 'missing 'none)"), "none");
}

#[test]
fn it_handles_escapes_and_unicode() {
    let mut h = h();
    assert_eq!(
        h.eval(r#"(json-decode "\"a\\nb\\tc\\\"d\\\\e\\u0041\\u00e9\"")"#),
        "\"a\\nb\\tc\\\"d\\\\eAé\""
    );
}

#[test]
fn it_encodes_objects_as_lists_and_arrays_as_vectors() {
    let mut h = h();
    assert_eq!(h.eval("(json-encode #t)"), "\"true\"");
    assert_eq!(h.eval("(json-encode 'null)"), "\"null\"");
    assert_eq!(h.eval("(json-encode 42)"), "\"42\"");
    // A whole float is written without a fraction, as JSON has one number type.
    assert_eq!(h.eval("(json-encode 3.0)"), "\"3\"");
    assert_eq!(h.eval("(json-encode \"a\\\"b\")"), r#""\"a\\\"b\"""#);
    assert_eq!(h.eval("(json-encode (json-array 1 2 3))"), "\"[1,2,3]\"");
    assert_eq!(h.eval("(json-encode (json-array))"), "\"[]\"");
    assert_eq!(h.eval("(json-encode (json-object))"), "\"{}\"");
    assert_eq!(
        h.eval("(json-encode (json-object 'a 1 'b (json-array \"x\")))"),
        r#""{\"a\":1,\"b\":[\"x\"]}""#
    );
}

#[test]
fn it_round_trips_a_request() {
    let mut h = h();
    h.eval(
        r#"(define req (json-object 'jsonrpc "2.0" 'id 0 'method "initialize"
             'params (json-object 'protocolVersion 1
                                  'clientCapabilities (json-object 'terminal #t)
                                  'mcpServers (json-array))))"#,
    );
    // Encoding then decoding gives the same fields back.
    h.eval("(define back (json-decode (json-encode req)))");
    assert_eq!(h.eval("(json-ref back 'method)"), "\"initialize\"");
    assert_eq!(
        h.eval("(json-ref (json-ref back 'params) 'protocolVersion)"),
        "1"
    );
    assert_eq!(
        h.eval("(json-ref (json-ref (json-ref back 'params) 'clientCapabilities) 'terminal)"),
        "#t"
    );
    assert_eq!(
        h.eval("(vector-length (json-ref (json-ref back 'params) 'mcpServers))"),
        "0"
    );
}
