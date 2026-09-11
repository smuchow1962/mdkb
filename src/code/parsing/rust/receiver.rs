//! The type of the expression a Rust method call was made on.
//!
//! A method call names its method and nothing else, and that is where the
//! whole residual ambiguity of the call graph sits: over the tier-7 edges of
//! this repository, a receiver written as a plain name carries 36 950 of the
//! 43 218 candidate slots — 2 701 edges at 13.68 candidates each. Naming the
//! receiver's type is what turns `temp.path()` from "every `path` in the
//! index" into one answer, or into an honest "this call leaves the index".
//!
//! What one file can answer is answered here. What needs the index — the
//! return type of a function declared elsewhere — is named as
//! [`ReceiverType::ReturnOf`](crate::code::parsing::parser::ReceiverType::ReturnOf) and resolved after every file is in.
//!
//! **Covered shapes**, each measured to matter:
//!
//! | The receiver | The type comes from |
//! |---|---|
//! | `x` bound by `let x: T` | the annotation |
//! | `x` bound by `let x = T::new()`, `let x = T { .. }` | the path or struct name |
//! | `x` bound by `let x = f()` | what `f` returns, once the index is complete |
//! | `x` declared as a parameter `x: T` | the signature |
//! | `f()`, `f().unwrap()` | what `f` returns, once the index is complete |
//!
//! **Not covered**, and left to the cascade that placed these edges before:
//! `self` and `self.field`, because a call on `self` is in the same file as
//! its own `impl` and tier 4 already resolves it at a fan-out of 1.03; a
//! binding from a `for` loop, a closure parameter or a `match` arm, because
//! none of them writes a type; an index or a literal receiver; and a receiver
//! produced by a method rather than a function — `x.build().close()` — because
//! `build` alone does not say which `build`.

use tree_sitter::Node;

use crate::code::parsing::parser::{ReceiverType, check_recursion_depth};

/// The type a Rust signature returns, reduced to the name a symbol is indexed
/// under.
///
/// `fn open(p: &Path) -> anyhow::Result<Store>` answers `Store`: the same
/// reduction [`type_name`] applies to an annotation, through the same code, so
/// a receiver typed from a `let` and one typed from a return type can never
/// disagree. `None` for a signature that returns nothing, or nothing this
/// index could hold a method for.
///
/// The signature is parsed rather than scanned for `->`: a return type can
/// hold one (`-> Box<dyn Fn() -> u32>`), and a parameter can too.
pub fn return_type(signature: &str) -> Option<String> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .ok()?;
    // A signature is stored without a body, which is not a parseable item.
    let source = format!("{signature} {{}}");
    let tree = parser.parse(&source, None)?;
    let item = tree
        .root_node()
        .named_child(0)
        .filter(|n| n.kind() == "function_item")?;
    let declared = item.child_by_field_name("return_type")?;
    type_name(declared, &source).map(ToOwned::to_owned)
}

/// The type of `receiver`, which was written inside `fn_node`.
///
/// `None` when this file does not say, which leaves the edge exactly where it
/// was: every same-named candidate, placed by the tier that placed it before.
/// Answering "unknown" with a guess is the one outcome worth avoiding, because
/// a wrong type reports a call as leaving the index when it did not.
pub fn infer<'a>(fn_node: Option<Node>, code: &'a str, receiver: Node) -> Option<ReceiverType<'a>> {
    let inferred = match receiver.kind() {
        // A name: the type is wherever the name was bound.
        "identifier" => binding_type(fn_node?, code, &code[receiver.byte_range()], receiver),
        // Anything that produces a value: the type is what produced it.
        _ => value_type(receiver, code),
    }?;
    match inferred {
        // `Self` is not a name anything is indexed under; it stands for the
        // type of the `impl` the call was written in, which is a lookup away.
        ReceiverType::Named("Self") => Some(ReceiverType::Named(self_type(fn_node?, code)?)),
        other => Some(other),
    }
}

/// The type the `impl` around `fn_node` is for — what `Self` means there.
fn self_type<'a>(fn_node: Node, code: &'a str) -> Option<&'a str> {
    let mut current = fn_node;
    for depth in 0.. {
        if !check_recursion_depth(depth, current) {
            return None;
        }
        if current.kind() == "impl_item" {
            return type_name(current.child_by_field_name("type")?, code);
        }
        current = current.parent()?;
    }
    None
}

/// The type of the value `node` evaluates to.
///
/// `.unwrap()` and its family are peeled first: `TempDir::new().unwrap()` is
/// a `TempDir` to every reader, and the wrapper it came out of is not a type
/// this index holds methods for.
fn value_type<'a>(node: Node, code: &'a str) -> Option<ReceiverType<'a>> {
    let node = peel_wrappers(node, code, 0)?;
    match node.kind() {
        // `Store { .. }` is a Store.
        "struct_expression" => Some(ReceiverType::Named(last_path_segment(
            &code[node.child_by_field_name("name")?.byte_range()],
        ))),
        "call_expression" => {
            let callee = node.child_by_field_name("function")?;
            match callee.kind() {
                // `TempDir::new()` reaches an associated function through the
                // type it belongs to, and that type is the answer without
                // asking the index anything. `tempfile::tempdir()` is the same
                // shape and means something else: the path is a module, so the
                // callee is a plain function and only its return type answers.
                //
                // Case is what tells the two apart. It is a convention rather
                // than grammar, and it is the convention `non_camel_case_types`
                // warns on by default, so Rust source that breaks it is rarer
                // than the 533 edges reading `tempfile::tempdir()` that being
                // wrong here mistyped as a `tempfile`.
                "scoped_identifier" => {
                    let path =
                        last_path_segment(&code[callee.child_by_field_name("path")?.byte_range()]);
                    Some(if path.starts_with(char::is_uppercase) {
                        ReceiverType::Named(path)
                    } else {
                        ReceiverType::ReturnOf(
                            &code[callee.child_by_field_name("name")?.byte_range()],
                        )
                    })
                }
                // `tempdir()`: only the index knows what it returns, and the
                // name is enough to ask.
                "identifier" => Some(ReceiverType::ReturnOf(&code[callee.byte_range()])),
                // `x.build()` is deliberately unknown. A method's name does
                // not identify a method — that is the ambiguity this whole
                // module exists to remove, and answering it from the name
                // alone reintroduces it one step further back. Measured on
                // this repository: `Command::args` resolved to the `Vec` a
                // free function named `args` returns, `HashMap::entry` to a
                // `MemoryEntry`, `Command::stdout` to a `String`. Against
                // that, the shape gave 6 edges a target.
                _ => None,
            }
        }
        _ => None,
    }
}

/// Strip the calls and operators that hand back what they were given.
///
/// `unwrap`, `expect` and `?` take a `Result` or an `Option` apart, `&`
/// borrows and `clone` copies. None of them changes which type's methods the
/// result answers to, and every one of them stands between the call site and
/// the name of that type: `TempDir::new().unwrap()` is a `TempDir`, and
/// `unwrap` is not a type this index holds methods for.
///
/// A loop with a depth guard, not a recursion: a chain of these nests once per
/// call, and a generated source can nest it past any stack.
fn peel_wrappers<'t>(node: Node<'t>, code: &str, depth: usize) -> Option<Node<'t>> {
    const TRANSPARENT: [&str; 5] = ["unwrap", "expect", "unwrap_or_default", "clone", "as_ref"];

    let mut current = node;
    for step in 0.. {
        if !check_recursion_depth(depth + step, current) {
            return None;
        }
        let next = match current.kind() {
            "try_expression" | "reference_expression" | "parenthesized_expression" => {
                current.named_child(0)
            }
            // `x.unwrap()`: the receiver of a transparent method, not the
            // method itself. A call to anything else is a value of its own and
            // the peeling stops there.
            "call_expression" => current
                .child_by_field_name("function")
                .filter(|callee| callee.kind() == "field_expression")
                .filter(|callee| {
                    callee
                        .child_by_field_name("field")
                        .is_some_and(|field| TRANSPARENT.contains(&&code[field.byte_range()]))
                })
                .and_then(|callee| callee.child_by_field_name("value")),
            _ => None,
        };
        match next {
            Some(inner) => current = inner,
            None => return Some(current),
        }
    }
    Some(current)
}

/// The type bound to `name` in the function it was written in.
///
/// A `let` that comes after the call site is not the binding the call used, so
/// the search is bounded by `used_at`: the nearest binding above the use wins,
/// which is what shadowing means.
fn binding_type<'a>(
    fn_node: Node,
    code: &'a str,
    name: &str,
    used_at: Node,
) -> Option<ReceiverType<'a>> {
    if let Some(found) = parameter_type(fn_node, code, name) {
        return Some(found);
    }
    let mut best: Option<(usize, ReceiverType<'a>)> = None;
    let mut stack = vec![(fn_node, 0usize)];
    while let Some((node, depth)) = stack.pop() {
        if !check_recursion_depth(depth, node) {
            continue;
        }
        if node.kind() == "let_declaration"
            && node.start_byte() < used_at.start_byte()
            && node
                .child_by_field_name("pattern")
                .is_some_and(|p| p.kind() == "identifier" && &code[p.byte_range()] == name)
        {
            let inferred = match node.child_by_field_name("type") {
                Some(annotation) => type_name(annotation, code).map(ReceiverType::Named),
                None => node
                    .child_by_field_name("value")
                    .and_then(|value| value_type(value, code)),
            };
            if let Some(inferred) = inferred {
                let at = node.start_byte();
                if best.is_none_or(|(seen, _)| at > seen) {
                    best = Some((at, inferred));
                }
            }
        }
        for child in node.children(&mut node.walk()) {
            if child.is_named() {
                stack.push((child, depth + 1));
            }
        }
    }
    best.map(|(_, inferred)| inferred)
}

/// The type of the parameter named `name`, out of the signature.
fn parameter_type<'a>(fn_node: Node, code: &'a str, name: &str) -> Option<ReceiverType<'a>> {
    let parameters = fn_node.child_by_field_name("parameters")?;
    for parameter in parameters.children(&mut parameters.walk()) {
        if parameter.kind() != "parameter" {
            continue;
        }
        let bound = parameter.child_by_field_name("pattern")?;
        if bound.kind() == "identifier" && &code[bound.byte_range()] == name {
            return type_name(parameter.child_by_field_name("type")?, code)
                .map(ReceiverType::Named);
        }
    }
    None
}

/// The name of the type `node` writes, `None` for one that names no type this
/// index could hold a method for.
///
/// `&Context` and `Context` are the same type to a method call, so a reference
/// is followed. A primitive is not followed by anything: `u32` has no method
/// in this index, and reporting the call as external is the right answer,
/// which is what an unmatched name already produces.
fn type_name<'a>(node: Node, code: &'a str) -> Option<&'a str> {
    let mut current = node;
    for depth in 0.. {
        if !check_recursion_depth(depth, current) {
            return None;
        }
        match current.kind() {
            "type_identifier" => return Some(&code[current.byte_range()]),
            "scoped_type_identifier" => {
                return Some(last_path_segment(&code[current.byte_range()]));
            }
            // `&T`, `&mut T`, `(T)`: the same type as far as a method goes.
            // By field, not by first child: the first named child of
            // `&mut T` is the `mutable_specifier`, which names no type.
            "reference_type" => current = current.child_by_field_name("type")?,
            "parenthesized_type" => current = current.named_child(0)?,
            // `Result<T>`, `Option<T>`, `Box<T>`, `Arc<T>`: see
            // [`unwrap_transparent`].
            "generic_type" => current = unwrap_transparent(current, code)?,
            _ => return None,
        }
    }
    None
}

/// The type inside a wrapper whose methods this index never holds.
///
/// `Result<Store>` and `Arc<Store>` both answer `Store`'s methods at the call
/// site — the first after an `unwrap` the parser has already peeled, the
/// second through `Deref`. Neither `Result` nor `Arc` is a type this index
/// declares, so looking for the method on the wrapper can only find nothing.
///
/// The list is deliberately short and does **not** include collections.
/// `Vec<Symbol>` is not a `Symbol`: `len` on a `Vec` is external, and reducing
/// the type to `Symbol` would resolve that call to `Symbol::len` if this index
/// happened to declare one. A wrong target is worse than an unplaced call.
fn unwrap_transparent<'t>(generic: Node<'t>, code: &str) -> Option<Node<'t>> {
    const TRANSPARENT: [&str; 6] = ["Result", "Option", "Box", "Arc", "Rc", "Cow"];

    let base = generic.child_by_field_name("type")?;
    let named = last_path_segment(&code[base.byte_range()]);
    if !TRANSPARENT.contains(&named) {
        return Some(base);
    }
    let arguments = generic.child_by_field_name("type_arguments")?;
    arguments
        .children(&mut arguments.walk())
        .find(|c| c.is_named())
}

/// `std::io::Result` → `Result`, `Store` → `Store`.
///
/// Symbols are indexed and matched by bare name, so a qualified type has to be
/// reduced to the segment it is indexed under.
fn last_path_segment(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path).trim()
}

#[cfg(test)]
mod tests {
    use super::infer;
    use crate::code::parsing::parser::ReceiverType;

    /// The type of the receiver of the **last** call in `code`.
    ///
    /// The last one, because every case below writes the bindings first and
    /// the call that reads them last, which is also the order a reader reads.
    fn receiver_type(code: &str) -> Option<ReceiverType<'_>> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        // The tree has to outlive the borrow, and `infer` borrows only `code`,
        // so the nodes are collected first and the answer is taken second.
        let (function, receiver) = last_method_call(tree.root_node(), code);
        infer(function, code, receiver)
    }

    /// The enclosing `function_item` and the receiver node of the last method
    /// call written in `code`.
    fn last_method_call<'t>(
        root: tree_sitter::Node<'t>,
        code: &str,
    ) -> (Option<tree_sitter::Node<'t>>, tree_sitter::Node<'t>) {
        let mut found = None;
        let mut stack = vec![(root, None)];
        while let Some((node, function)) = stack.pop() {
            let function = if node.kind() == "function_item" {
                Some(node)
            } else {
                function
            };
            if node.kind() == "call_expression" {
                if let Some(callee) = node
                    .child_by_field_name("function")
                    .filter(|c| c.kind() == "field_expression")
                {
                    if let Some(value) = callee.child_by_field_name("value") {
                        let at = node.start_byte();
                        if found.is_none_or(|(seen, _, _)| at > seen) {
                            found = Some((at, function, value));
                        }
                    }
                }
            }
            for child in node.children(&mut node.walk()) {
                if child.is_named() {
                    stack.push((child, function));
                }
            }
        }
        let (_, function, receiver) = found.unwrap_or_else(|| panic!("no method call in:\n{code}"));
        (function, receiver)
    }

    /// The shape carrying 36 950 of the 43 218 residual candidate slots: a
    /// local whose type is written in the code that binds it.
    #[test]
    fn a_local_bound_to_a_named_type_takes_that_type() {
        assert_eq!(
            receiver_type("fn f() { let s: Store = make(); s.get(); }"),
            Some(ReceiverType::Named("Store")),
            "an annotation says the type outright"
        );
        assert_eq!(
            receiver_type("fn f() { let s = Store::open(); s.get(); }"),
            Some(ReceiverType::Named("Store")),
            "an associated function is reached through its own type"
        );
        assert_eq!(
            receiver_type("fn f() { let s = Store { x: 1 }; s.get(); }"),
            Some(ReceiverType::Named("Store")),
            "a struct literal is that struct"
        );
    }

    /// `TempDir::new().unwrap()` is a `TempDir`, and 95 edges in this
    /// repository are that exact line. Without peeling, the type would be read
    /// off `unwrap`, which names nothing.
    #[test]
    fn unwrapping_a_constructor_still_names_the_type() {
        assert_eq!(
            receiver_type("fn f() { let d = TempDir::new().unwrap(); d.path(); }"),
            Some(ReceiverType::Named("TempDir"))
        );
        assert_eq!(
            receiver_type("fn f() { let d = TempDir::new().expect(\"nope\"); d.path(); }"),
            Some(ReceiverType::Named("TempDir"))
        );
        assert_eq!(
            receiver_type("fn f() -> X { let d = TempDir::new()?; d.path(); }"),
            Some(ReceiverType::Named("TempDir"))
        );
        assert_eq!(
            receiver_type("fn f() { let d = &TempDir::new().unwrap(); d.path(); }"),
            Some(ReceiverType::Named("TempDir"))
        );
    }

    /// 385 edges in this repository read `tempfile::tempdir().unwrap()` and
    /// 325 read a local test helper. Neither names a type at the call site, so
    /// the answer is the function's name and the index resolves it later.
    #[test]
    fn a_local_built_by_a_call_names_the_call() {
        assert_eq!(
            receiver_type("fn f() { let d = tempdir().unwrap(); d.path(); }"),
            Some(ReceiverType::ReturnOf("tempdir"))
        );
        assert_eq!(
            receiver_type("fn f() { let d = setup_temp_dir(); d.path(); }"),
            Some(ReceiverType::ReturnOf("setup_temp_dir"))
        );
        assert_eq!(
            receiver_type("fn f() { let d = tempfile::tempdir().unwrap(); d.path(); }"),
            Some(ReceiverType::ReturnOf("tempdir")),
            "the 533 edges that read a module path as a type named `tempfile`"
        );
    }

    /// A parameter's type is in the signature, and 44 % of the ambiguous
    /// local receivers measured are parameters rather than `let` bindings.
    #[test]
    fn a_parameter_takes_the_type_its_signature_declares() {
        assert_eq!(
            receiver_type("fn f(ctx: &Context) { ctx.get(); }"),
            Some(ReceiverType::Named("Context")),
            "a reference is the same type to a method call"
        );
        assert_eq!(
            receiver_type("fn f(ctx: &mut crate::ctx::Context) { ctx.get(); }"),
            Some(ReceiverType::Named("Context")),
            "a qualified type is indexed under its last segment"
        );
        assert_eq!(
            receiver_type("fn f(s: Arc<Store>) { s.get(); }"),
            Some(ReceiverType::Named("Store")),
            "Arc holds no method this index declares, and Deref reaches through it"
        );
        assert_eq!(
            receiver_type("fn f(s: Result<Store>) { s.get(); }"),
            Some(ReceiverType::Named("Store")),
            "a Result is unwrapped before a method of the index is reached"
        );
    }

    /// `Vec<Symbol>` is not a `Symbol`. Reducing it would resolve `len` to
    /// `Symbol::len` if the index declared one — a wrong target, which is
    /// worse than the unplaced call this leaves behind.
    #[test]
    fn a_collection_is_not_reduced_to_what_it_holds() {
        assert_eq!(
            receiver_type("fn f(items: Vec<Symbol>) { items.len(); }"),
            Some(ReceiverType::Named("Vec"))
        );
        assert_eq!(
            receiver_type("fn f(by_name: HashMap<String, Symbol>) { by_name.get(); }"),
            Some(ReceiverType::Named("HashMap"))
        );
    }

    /// The nearest binding above the use is the one the call used. Answering
    /// with the later one would name a type the call never saw.
    #[test]
    fn a_shadowed_local_takes_the_binding_above_the_call() {
        assert_eq!(
            receiver_type(
                "fn f() {\n\
                 \x20   let s = First::new();\n\
                 \x20   let s = Second::new();\n\
                 \x20   s.get();\n\
                 }"
            ),
            Some(ReceiverType::Named("Second"))
        );
        assert_eq!(
            receiver_type(
                "fn f() {\n\
                 \x20   let s = First::new();\n\
                 \x20   s.get();\n\
                 \x20   let s = Second::new();\n\
                 }"
            ),
            Some(ReceiverType::Named("First")),
            "a binding written after the call is not the one it read"
        );
    }

    /// `Self` is a name no symbol is indexed under, so reading it as a type
    /// reports the call as leaving the index. It is a lookup away: the `impl`
    /// the call sits in says which type it is.
    #[test]
    fn self_is_read_as_the_type_the_impl_is_for() {
        assert_eq!(
            receiver_type("impl Store { fn f() { let s = Self::open(); s.get(); } }"),
            Some(ReceiverType::Named("Store"))
        );
        assert_eq!(
            receiver_type("impl Deref for Store { fn f() { let s: Self = make(); s.get(); } }"),
            Some(ReceiverType::Named("Store")),
            "the impl's type, not the trait it implements"
        );
        assert_eq!(
            receiver_type("fn f() { let s = Self::open(); s.get(); }"),
            None,
            "outside an impl, Self stands for nothing this file names"
        );
    }

    /// What this file cannot answer has to stay unanswered: a guess would
    /// report a call as leaving the index when it did not.
    #[test]
    fn a_receiver_this_file_does_not_explain_stays_unknown() {
        assert_eq!(
            receiver_type("fn f() { for item in list { item.go(); } }"),
            None,
            "a for binding writes no type"
        );
        assert_eq!(
            receiver_type("fn f() { rows.map(|row| row.get()); }"),
            None,
            "a closure parameter writes no type"
        );
        assert_eq!(
            receiver_type("fn f(&self) { self.db.get(); }"),
            None,
            "a field receiver needs field types, which the index does not hold"
        );
        assert_eq!(
            receiver_type("fn f() { items[0].go(); }"),
            None,
            "an element of an unknown collection is unknown"
        );
    }

    /// A chain off a function is placed by that function's name. A chain off a
    /// method is not placed at all: `handle` names a method, and a method name
    /// matched against every same-named symbol is the ambiguity this module
    /// removes. Measured on this repository, doing it anyway typed
    /// `Command::args(..)` as the `Vec` an unrelated `args` returns.
    #[test]
    fn a_chained_receiver_is_placed_only_when_a_function_produced_it() {
        assert_eq!(
            receiver_type("fn f() { build().close(); }"),
            Some(ReceiverType::ReturnOf("build"))
        );
        assert_eq!(
            receiver_type("fn f() { make::build().close(); }"),
            Some(ReceiverType::ReturnOf("build")),
            "a lower-case path is a module, so the callee is a plain function"
        );
        assert_eq!(receiver_type("fn f() { store.handle().close(); }"), None);
    }
}
