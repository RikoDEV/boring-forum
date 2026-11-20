//! Fluent patterns compiled to the small tree the browser interprets. Message and term references
//! are inlined here, so the script needs no catalog lookups; anything the script could not render
//! (other functions, attributes, term arguments) is an error, which fails the tests and rejects an
//! admin override rather than showing wrong text.

use fluent_syntax::ast::{Expression, InlineExpression, Pattern, PatternElement, VariantKey};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub(super) type Pat = Pattern<String>;

/// The messages and terms of one catalog, by Fluent id.
#[derive(Default)]
pub(super) struct Ast {
    pub messages: HashMap<String, Pat>,
    pub terms: HashMap<String, Pat>,
}

/// A catalog with an optional fallback behind it (English behind a translation).
#[derive(Clone, Copy)]
pub(super) struct Chain<'a> {
    pub first: &'a Ast,
    pub second: Option<&'a Ast>,
}

impl Chain<'_> {
    fn message(&self, id: &str) -> Option<&Pat> {
        self.first.messages.get(id).or_else(|| self.second.and_then(|a| a.messages.get(id)))
    }
    fn term(&self, id: &str) -> Option<&Pat> {
        self.first.terms.get(id).or_else(|| self.second.and_then(|a| a.terms.get(id)))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Part {
    Text(String),
    Var { name: String, num: bool },
    Sel { name: String, cases: Vec<(String, Vec<Part>)>, def: String },
}

/// Compiles a pattern, resolving references through `chain`.
pub(super) fn compile(pattern: &Pat, chain: Chain) -> Result<Vec<Part>, String> {
    let mut out = Vec::new();
    pattern_into(pattern, chain, &mut Vec::new(), false, &mut out)?;
    Ok(out)
}

fn push(out: &mut Vec<Part>, part: Part) {
    match (out.last_mut(), part) {
        (Some(Part::Text(last)), Part::Text(t)) => last.push_str(&t),
        (_, Part::Text(t)) if t.is_empty() => {}
        (_, part) => out.push(part),
    }
}

fn pattern_into(
    pattern: &Pat,
    chain: Chain,
    stack: &mut Vec<String>,
    in_term: bool,
    out: &mut Vec<Part>,
) -> Result<(), String> {
    for element in &pattern.elements {
        match element {
            PatternElement::TextElement { value } => push(out, Part::Text(value.clone())),
            PatternElement::Placeable { expression } => expression_into(expression, chain, stack, in_term, out)?,
        }
    }
    Ok(())
}

fn var_of(expr: &InlineExpression<String>) -> Option<(&str, bool)> {
    match expr {
        InlineExpression::VariableReference { id } => Some((&id.name, false)),
        InlineExpression::FunctionReference { id, arguments } if id.name == "NUMBER" => {
            match (arguments.positional.as_slice(), arguments.named.is_empty()) {
                ([InlineExpression::VariableReference { id }], true) => Some((&id.name, true)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn expression_into(
    expr: &Expression<String>,
    chain: Chain,
    stack: &mut Vec<String>,
    in_term: bool,
    out: &mut Vec<Part>,
) -> Result<(), String> {
    match expr {
        Expression::Inline(inline) => inline_into(inline, chain, stack, in_term, out),
        Expression::Select { selector, variants } => {
            let (name, _) = var_of(selector).ok_or("a select must test a variable or NUMBER($variable)")?;
            if in_term {
                return Err("a term cannot select on a variable".into());
            }
            let mut cases = Vec::new();
            let mut def = None;
            for variant in variants {
                let key = match &variant.key {
                    VariantKey::Identifier { name } => name.clone(),
                    VariantKey::NumberLiteral { value } => value.clone(),
                };
                let mut body = Vec::new();
                pattern_into(&variant.value, chain, stack, in_term, &mut body)?;
                if variant.default {
                    def = Some(key.clone());
                }
                cases.push((key, body));
            }
            let def = def.ok_or("a select needs a default variant")?;
            push(out, Part::Sel { name: name.to_owned(), cases, def });
            Ok(())
        }
    }
}

fn inline_into(
    expr: &InlineExpression<String>,
    chain: Chain,
    stack: &mut Vec<String>,
    in_term: bool,
    out: &mut Vec<Part>,
) -> Result<(), String> {
    match expr {
        InlineExpression::StringLiteral { value } => {
            push(out, Part::Text(super::string_literal(value)));
            Ok(())
        }
        InlineExpression::NumberLiteral { value } => {
            push(out, Part::Text(value.clone()));
            Ok(())
        }
        InlineExpression::VariableReference { id } => {
            if in_term {
                return Err(format!("a term cannot use the variable ${}", id.name));
            }
            push(out, Part::Var { name: id.name.clone(), num: false });
            Ok(())
        }
        InlineExpression::FunctionReference { id, .. } => match var_of(expr) {
            Some((name, true)) => {
                if in_term {
                    return Err("a term cannot use variables".into());
                }
                push(out, Part::Var { name: name.to_owned(), num: true });
                Ok(())
            }
            _ if id.name == "NUMBER" => Err("NUMBER takes one variable and no options".into()),
            _ => Err(format!("the function {}() is not supported", id.name)),
        },
        InlineExpression::MessageReference { id, attribute } => {
            if attribute.is_some() {
                return Err(format!("attributes ({}.…) are not supported", id.name));
            }
            let target = chain.message(&id.name).ok_or_else(|| format!("unknown message {}", id.name))?;
            enter(format!("m:{}", id.name), stack, |stack| pattern_into(target, chain, stack, in_term, out))
        }
        InlineExpression::TermReference { id, attribute, arguments } => {
            if attribute.is_some() || arguments.is_some() {
                return Err(format!("term -{} with attributes or arguments is not supported", id.name));
            }
            let target = chain.term(&id.name).ok_or_else(|| format!("unknown term -{}", id.name))?;
            enter(format!("t:{}", id.name), stack, |stack| pattern_into(target, chain, stack, true, out))
        }
        InlineExpression::Placeable { expression } => expression_into(expression, chain, stack, in_term, out),
    }
}

fn enter(
    frame: String,
    stack: &mut Vec<String>,
    body: impl FnOnce(&mut Vec<String>) -> Result<(), String>,
) -> Result<(), String> {
    if stack.contains(&frame) {
        return Err(format!("{} refers to itself", &frame[2..]));
    }
    stack.push(frame);
    let result = body(stack);
    stack.pop();
    result
}

/// Variables a compiled message uses, with whether any use is numeric (a select or NUMBER).
pub(super) fn variables(parts: &[Part]) -> Vec<(String, bool)> {
    fn walk(parts: &[Part], found: &mut Vec<(String, bool)>) {
        fn add(found: &mut Vec<(String, bool)>, name: &str, num: bool) {
            match found.iter_mut().find(|(n, _)| n == name) {
                Some(entry) => entry.1 |= num,
                None => found.push((name.to_owned(), num)),
            }
        }
        for part in parts {
            match part {
                Part::Text(_) => {}
                Part::Var { name, num } => add(found, name, *num),
                Part::Sel { name, cases, .. } => {
                    add(found, name, true);
                    for (_, body) in cases {
                        walk(body, found);
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(parts, &mut found);
    found
}

pub(super) fn to_json(parts: &[Part]) -> Value {
    match parts {
        [] => Value::String(String::new()),
        [Part::Text(t)] => Value::String(t.clone()),
        _ => Value::Array(
            parts
                .iter()
                .map(|p| match p {
                    Part::Text(t) => Value::String(t.clone()),
                    Part::Var { name, num: false } => json!({ "v": name }),
                    Part::Var { name, num: true } => json!({ "v": name, "num": true }),
                    Part::Sel { name, cases, def } => {
                        let cases: Map<String, Value> =
                            cases.iter().map(|(k, body)| (k.clone(), to_json(body))).collect();
                        json!({ "sel": name, "cases": cases, "def": def })
                    }
                })
                .collect(),
        ),
    }
}
