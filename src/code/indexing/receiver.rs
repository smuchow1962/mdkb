//! The receiver a call was made on, from the expression to a type.
//!
//! A method call names its method and nothing else: `get` matched every `get`
//! in this repository's index, 6.95 of them on average, and that one shape is
//! the whole of the residual ambiguity measured in
//! `plans/qualified-symbol-resolution.md`. The parser writes down the
//! expression the call was made on ([`Call::receiver`]), and this module
//! reduces that expression to a type, which the resolver can match against the
//! owner recorded on every symbol.
//!
//! [`Call::receiver`]: crate::code::parsing::parser::Call::receiver

/// The longest receiver expression kept, in bytes.
///
/// A receiver is normally a name or two. A multi-line builder chain is the
/// exception and can run to a paragraph, which does not belong in a column
/// read once per edge — and a minified source has no line breaks to bound it
/// at all.
const MAX_RECEIVER_BYTES: usize = 200;

/// The receiver expression as a single line, short enough to store.
///
/// Whitespace is collapsed because a chain written over four lines is one
/// expression, and the rows for `foo\n    .bar()` and `foo.bar()` have to
/// compare equal. `None` for an expression that is nothing but whitespace,
/// which no call site writes but a grammar can hand over.
///
/// Over the cap it is the **tail** that is kept, not the head: the type of a
/// chain is the return type of its last call, so the end of the expression is
/// the part that still carries information once the rest is gone.
pub fn normalize(expr: &str) -> Option<Box<str>> {
    let mut out = String::with_capacity(expr.len().min(MAX_RECEIVER_BYTES));
    let mut space = false;
    for ch in expr.chars() {
        if ch.is_whitespace() {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
            }
            space = false;
            out.push(ch);
        }
    }
    if out.is_empty() {
        return None;
    }
    if out.len() > MAX_RECEIVER_BYTES {
        let mut cut = out.len() - MAX_RECEIVER_BYTES;
        while cut < out.len() && !out.is_char_boundary(cut) {
            cut += 1;
        }
        out = out[cut..].to_string();
    }
    Some(out.into())
}

#[cfg(test)]
mod tests {
    use super::{MAX_RECEIVER_BYTES, normalize};

    /// A chain written over several lines is one expression, and has to be
    /// stored as the same text as the same chain written on one line.
    #[test]
    fn a_receiver_spanning_lines_is_stored_as_one_line() {
        assert_eq!(
            normalize("foo\n    .bar()\n    .baz()").as_deref(),
            Some("foo .bar() .baz()")
        );
        assert_eq!(normalize("self.db").as_deref(), Some("self.db"));
        assert_eq!(normalize("  temp  ").as_deref(), Some("temp"));
    }

    #[test]
    fn a_receiver_of_nothing_but_whitespace_is_not_a_receiver() {
        assert_eq!(normalize("   \n  "), None);
        assert_eq!(normalize(""), None);
    }

    /// The tail survives the cap, because the type of a chain is the return
    /// type of its last call. Keeping the head would store the part that
    /// cannot be resolved and drop the part that can.
    #[test]
    fn an_over_long_receiver_keeps_its_tail() {
        let long = format!("{}.last_call()", "a.step()".repeat(60));
        let stored = normalize(&long).expect("a long receiver is still a receiver");

        assert!(stored.len() <= MAX_RECEIVER_BYTES, "{}", stored.len());
        assert!(
            stored.ends_with(".last_call()"),
            "the tail names the call whose return type is the receiver's type: {stored}"
        );
    }

    /// The cap counts bytes, and cutting one in half would panic. A receiver
    /// of multi-byte characters is unusual and a parser will still hand one
    /// over.
    #[test]
    fn capping_a_multibyte_receiver_cuts_on_a_character_boundary() {
        let long = "é".repeat(MAX_RECEIVER_BYTES);
        let stored = normalize(&long).expect("still a receiver");

        assert!(stored.len() <= MAX_RECEIVER_BYTES);
        assert!(stored.chars().all(|c| c == 'é'), "{stored}");
    }
}
