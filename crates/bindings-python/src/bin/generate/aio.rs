use rustpython_parser::{
    ast::{self, Constant, Expr, Ranged as _, Stmt, StmtClassDef, StmtFunctionDef},
    Parse as _,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    path::Path,
};

// Native classes that perform I/O get an `Async` wrapper; every other class is returned as is.
const WRAPPED: [&str; 17] = [
    "ActionBuilder",
    "Diagnostics",
    "Events",
    "Evidence",
    "Instrumentation",
    "Locator",
    "Native",
    "OriginalRecording",
    "Output",
    "Pointer",
    "Recording",
    "Registry",
    "Screen",
    "Session",
    "System",
    "Termtrace",
    "Tmux",
];
// Hand-written base class providing special behavior, per wrapped class.
const BASES: [(&str, &str); 2] = [("Locator", "LocatorBase"), ("Recording", "RecordingBase")];
// Methods that run on the calling thread because they never block.
const IMMEDIATE: [(&str, &[&str]); 9] = [
    ("ActionBuilder", &["*"]),
    ("Events", &["cursor_to_json", "cursor_from_json"]),
    (
        "Locator",
        &[
            "and_",
            "in_row",
            "in_region",
            "with_style",
            "across_soft_wraps",
        ],
    ),
    ("OriginalRecording", &["session", "detach"]),
    ("Output", &["cursor_to_json", "cursor_from_json"]),
    (
        "Recording",
        &[
            "actions",
            "analysis",
            "events",
            "evidence",
            "instrumentation",
            "native",
            "output",
            "pointer",
            "screen",
        ],
    ),
    (
        "Screen",
        &[
            "locator",
            "get_by_text",
            "get_by_regex",
            "row",
            "region",
            "filter",
        ],
    ),
    (
        "Session",
        &[
            "actions",
            "analysis",
            "events",
            "evidence",
            "instrumentation",
            "native",
            "output",
            "pointer",
            "screen",
        ],
    ),
    (
        "Termtrace",
        &[
            "analysis",
            "evidence",
            "instrumentation",
            "native",
            "registry",
            "system",
            "tmux",
        ],
    ),
];
// Mutations keep running after a cancelled await and are tracked until they return.
const MUTATIONS: [(&str, &[&str]); 5] = [
    ("ActionBuilder", &["run", "execute"]),
    (
        "OriginalRecording",
        &["finish", "wait_finished", "force_close"],
    ),
    ("Pointer", &["*"]),
    (
        "Recording",
        &[
            "execute_action",
            "mark",
            "pointer_click",
            "pointer_focus",
            "pointer_scroll",
            "request_stop",
            "resize",
            "send",
            "signal",
            "wait_for_completion",
        ],
    ),
    ("Session", &["mark", "resize", "send", "signal"]),
];
// Launches force-close a recording that starts after its await was cancelled.
const LAUNCHES: [(&str, &[&str]); 1] = [("Termtrace", &["launch", "record"])];
// Wrappers whose mutations belong to the recording or session that created them.
const CHILDREN: [&str; 2] = ["ActionBuilder", "Pointer"];
const SKIPPED: [&str; 5] = ["__enter__", "__exit__", "__repr__", "__new__", "__init__"];
const HEADER: [&str; 29] = [
    r#""""Asynchronous Termtrace wrappers. Generated from the native stub; run `just generate`.""""#,
    "",
    "from __future__ import annotations",
    "",
    "import asyncio",
    "import builtins",
    "import os",
    "import typing",
    "from collections.abc import Coroutine",
    "from typing import TYPE_CHECKING, Any",
    "",
    "from termtrace import _termtrace as _native",
    "from termtrace import types",
    "from termtrace._async import (",
    "    LocatorBase,",
    "    RecordingBase,",
    "    Wrapper,",
    "    launch,",
    "    offload_mutation,",
    "    run_cancellable,",
    ")",
    "from termtrace._termtrace import *  # noqa: F403",
    "from termtrace._termtrace import CancelToken",
    "",
    "if TYPE_CHECKING:",
    "    from typing_extensions import Self",
    "",
    "    from termtrace import editing",
    "",
];

type Failure = Box<dyn Error>;

pub(crate) fn write(package: &Path, stub: &str) -> Result<(), Failure> {
    let runtime = fs_err::read_to_string(package.join("_async.py"))?;
    let output = generate(stub, &runtime)?;
    fs_err::write(package.join("aio.py"), output)?;

    return Ok(());
}

fn listed(table: &[(&str, &[&str])], class: &str, name: &str) -> bool {
    return table
        .iter()
        .filter(|(owner, _)| return *owner == class)
        .any(|(_, names)| return names.contains(&name) || names.contains(&"*"));
}

fn base(class: &str) -> Option<&'static str> {
    return BASES
        .iter()
        .find(|(owner, _)| return *owner == class)
        .map(|(_, base)| return *base);
}

struct Source<'text> {
    text: &'text str,
}

impl Source<'_> {
    fn slice(&self, expr: &Expr) -> Result<&str, Failure> {
        let range = expr.range();

        return Ok(self
            .text
            .get(range.start().to_usize()..range.end().to_usize())
            .ok_or("expression range outside the stub")?);
    }

    fn annotation(&self, expr: Option<&Expr>) -> Result<String, Failure> {
        let Some(expr) = expr else {
            return Ok(String::new());
        };

        let mut names = Vec::new();
        wrapped_names(expr, &mut names)?;

        let start = expr.range().start().to_usize();
        let mut text = self.slice(expr)?.to_owned();

        for (offset, name) in names.into_iter().rev() {
            let at = offset - start;
            text.replace_range(at..at + name.len(), &format!("Async{name}"));
        }

        return Ok(text);
    }

    fn parameters(&self, function: &StmtFunctionDef) -> Result<(String, Vec<String>), Failure> {
        let arguments = &function.args;
        let mut rendered = vec!["self".to_owned()];
        let mut forwarded = Vec::new();
        let positional = arguments.posonlyargs.iter().chain(&arguments.args).skip(1);

        for argument in positional {
            rendered.push(self.parameter(argument)?);
            forwarded.push(argument.def.arg.to_string());
        }

        if !arguments.kwonlyargs.is_empty() {
            rendered.push("*".to_owned());

            for argument in &arguments.kwonlyargs {
                rendered.push(self.parameter(argument)?);
                forwarded.push(format!("{0}={0}", argument.def.arg));
            }
        }

        return Ok((rendered.join(", "), forwarded));
    }

    fn parameter(&self, argument: &ast::ArgWithDefault) -> Result<String, Failure> {
        let mut text = argument.def.arg.to_string();
        if let Some(annotation) = &argument.def.annotation {
            text = format!("{text}: {}", self.annotation(Some(annotation))?);
        }

        if let Some(default) = &argument.default {
            text = format!("{text} = {}", self.slice(default)?);
        }

        return Ok(text);
    }
}

fn wrapped_names(expr: &Expr, names: &mut Vec<(usize, String)>) -> Result<(), Failure> {
    match expr {
        Expr::Name(name) => {
            if WRAPPED.contains(&name.id.as_str()) {
                names.push((name.range.start().to_usize(), name.id.to_string()));
            }
        }
        Expr::Attribute(attribute) => wrapped_names(&attribute.value, names)?,
        Expr::Subscript(subscript) => {
            wrapped_names(&subscript.value, names)?;
            wrapped_names(&subscript.slice, names)?;
        }
        Expr::BinOp(operation) => {
            wrapped_names(&operation.left, names)?;
            wrapped_names(&operation.right, names)?;
        }
        Expr::Tuple(tuple) => {
            for item in &tuple.elts {
                wrapped_names(item, names)?;
            }
        }
        Expr::List(list) => {
            for item in &list.elts {
                wrapped_names(item, names)?;
            }
        }
        Expr::Constant(_) => {}
        _ => return Err(format!("unsupported annotation expression at {:?}", expr.range()).into()),
    }

    return Ok(());
}

fn returned(expr: Option<&Expr>) -> Option<String> {
    let Some(Expr::Name(name)) = expr else {
        return None;
    };

    return WRAPPED
        .contains(&name.id.as_str())
        .then(|| return name.id.to_string());
}

fn decorated(function: &StmtFunctionDef, decorator: &str) -> bool {
    return function
        .decorator_list
        .iter()
        .any(|expr| return matches!(expr, Expr::Name(name) if name.id.as_str() == decorator));
}

fn docstring_text(body: &[Stmt]) -> Option<String> {
    let Some(Stmt::Expr(statement)) = body.first() else {
        return None;
    };
    let Expr::Constant(constant) = statement.value.as_ref() else {
        return None;
    };
    let Constant::Str(text) = &constant.value else {
        return None;
    };

    return Some(clean_docstring(text));
}

// Python's `inspect.cleandoc`, which `ast.get_docstring` applies.
fn clean_docstring(text: &str) -> String {
    let lines = text.replace('\t', "        ");
    let lines = lines.lines().collect::<Vec<_>>();
    let indent = lines
        .iter()
        .skip(1)
        .filter(|line| return !line.trim_start().is_empty())
        .map(|line| return line.len() - line.trim_start().len())
        .min()
        .unwrap_or_default();
    let mut cleaned = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                return line.trim_start().to_owned();
            }
            return line
                .get(indent..)
                .unwrap_or_else(|| return line.trim_start())
                .to_owned();
        })
        .collect::<Vec<_>>();

    while cleaned.last().is_some_and(String::is_empty) {
        cleaned.pop();
    }

    while cleaned.first().is_some_and(String::is_empty) {
        cleaned.remove(0);
    }

    return cleaned.join("\n");
}

fn docstring(function: &StmtFunctionDef, indent: &str) -> Vec<String> {
    let Some(text) = docstring_text(&function.body).filter(|text| return !text.is_empty()) else {
        return Vec::new();
    };
    let lines = text.trim().lines().collect::<Vec<_>>();
    let [first, rest @ ..] = lines.as_slice() else {
        return Vec::new();
    };
    if rest.is_empty() {
        return vec![format!("{indent}r\"\"\"{first}\"\"\"")];
    }

    let mut out = vec![format!("{indent}r\"\"\"{first}")];
    out.extend(
        rest.iter()
            .map(|line| return format!("{indent}{line}").trim_end().to_owned()),
    );
    out.push(format!("{indent}\"\"\""));

    return out;
}

fn wrap(expression: &str, class: Option<&str>) -> String {
    let Some(class) = class else {
        return expression.to_owned();
    };
    let pending = if CHILDREN.contains(&class) { ", self._pending" } else { "" };

    return format!("Async{class}._wrap({expression}{pending})");
}

fn method(
    source: &Source<'_>,
    class: &str,
    function: &StmtFunctionDef,
) -> Result<Vec<String>, Failure> {
    let name = function.name.as_str();
    let (signature, forwarded) = source.parameters(function)?;
    let returns = function.returns.as_deref();
    let result = source.annotation(returns)?;
    let result = if result.is_empty() { "None".to_owned() } else { result };
    let target = returned(returns);
    let target = target.as_deref();

    let call = format!("self._sync.{name}({})", forwarded.join(", "));
    let mut lines = Vec::new();

    if decorated(function, "property") {
        lines.push("    @property".to_owned());
        lines.push(format!("    def {name}(self) -> {result}:"));
        lines.extend(docstring(function, "        "));
        lines.push(format!(
            "        return {}",
            wrap(&format!("self._sync.{name}"), target)
        ));

        return Ok(lines);
    }

    let cancellable = function
        .args
        .args
        .iter()
        .chain(&function.args.kwonlyargs)
        .any(|argument| return argument.def.arg.as_str() == "cancel");

    if class == "ActionBuilder" && name == "run" {
        // Sealing happens now, so the request cannot change while it is in flight.
        lines.push(format!(
            "    def {name}({signature}) -> Coroutine[Any, Any, {result}]:"
        ));
        lines.extend(docstring(function, "        "));
        lines.push("        self._sync.seal()".to_owned());
        lines.push(format!(
            "        return offload_mutation(self._pending, lambda: {call})"
        ));

        return Ok(lines);
    }

    if listed(&IMMEDIATE, class, name) && !listed(&MUTATIONS, class, name) {
        let shown = if target == Some(class) { "Self" } else { &result };
        lines.push(format!("    def {name}({signature}) -> {shown}:"));
        lines.extend(docstring(function, "        "));
        if target == Some(class) {
            lines.push(format!("        {call}"));
            lines.push("        return self".to_owned());
        } else {
            lines.push(format!("        return {}", wrap(&call, target)));
        }

        return Ok(lines);
    }

    lines.push(format!("    async def {name}({signature}) -> {result}:"));
    lines.extend(docstring(function, "        "));
    if cancellable {
        let tokened = forwarded
            .iter()
            .map(|argument| {
                return match argument.as_str() {
                    "cancel=cancel" => "cancel=token",
                    "cancel" => "token",
                    other => other,
                };
            })
            .collect::<Vec<_>>();

        lines.push("        def call(token: CancelToken) -> typing.Any:".to_owned());
        lines.push(format!(
            "            return self._sync.{name}({})",
            tokened.join(", ")
        ));
        lines.push(String::new());
        lines.push(format!(
            "        return {}",
            wrap("await run_cancellable(call, cancel)", target)
        ));
    } else if listed(&LAUNCHES, class, name) {
        lines.push(format!(
            "        return {}",
            wrap(&format!("await launch(lambda: {call})"), target)
        ));
    } else if listed(&MUTATIONS, class, name) {
        let awaited = format!("await offload_mutation(self._pending, lambda: {call})");
        lines.push(format!("        return {}", wrap(&awaited, target)));
    } else {
        let awaited = format!("await asyncio.to_thread(lambda: {call})");
        lines.push(format!("        return {}", wrap(&awaited, target)));
    }

    return Ok(lines);
}

fn constructor(
    source: &Source<'_>,
    class: &str,
    function: Option<&StmtFunctionDef>,
) -> Result<Vec<String>, Failure> {
    let Some(function) = function else {
        return Ok(Vec::new());
    };
    let (signature, forwarded) = source.parameters(function)?;

    return Ok(vec![
        format!("    def __init__({signature}) -> None:"),
        format!(
            "        self._sync = _native.{class}({})",
            forwarded.join(", ")
        ),
        "        self._pending = set()".to_owned(),
        String::new(),
    ]);
}

fn collect_functions(body: &[Stmt], names: &mut BTreeSet<String>) {
    for statement in body {
        match statement {
            Stmt::FunctionDef(function) => {
                names.insert(function.name.to_string());
                collect_functions(&function.body, names);
            }
            Stmt::AsyncFunctionDef(function) => {
                names.insert(function.name.to_string());
                collect_functions(&function.body, names);
            }
            Stmt::ClassDef(class) => collect_functions(&class.body, names),
            Stmt::If(branch) => {
                collect_functions(&branch.body, names);
                collect_functions(&branch.orelse, names);
            }
            Stmt::For(looped) => {
                collect_functions(&looped.body, names);
                collect_functions(&looped.orelse, names);
            }
            Stmt::AsyncFor(looped) => {
                collect_functions(&looped.body, names);
                collect_functions(&looped.orelse, names);
            }
            Stmt::While(looped) => {
                collect_functions(&looped.body, names);
                collect_functions(&looped.orelse, names);
            }
            Stmt::With(with) => collect_functions(&with.body, names),
            Stmt::AsyncWith(with) => collect_functions(&with.body, names),
            Stmt::Try(attempt) => {
                collect_functions(&attempt.body, names);

                for ast::ExceptHandler::ExceptHandler(handler) in &attempt.handlers {
                    collect_functions(&handler.body, names);
                }

                collect_functions(&attempt.orelse, names);
                collect_functions(&attempt.finalbody, names);
            }
            Stmt::TryStar(attempt) => {
                collect_functions(&attempt.body, names);

                for ast::ExceptHandler::ExceptHandler(handler) in &attempt.handlers {
                    collect_functions(&handler.body, names);
                }

                collect_functions(&attempt.orelse, names);
                collect_functions(&attempt.finalbody, names);
            }
            Stmt::Match(matched) => {
                for case in &matched.cases {
                    collect_functions(&case.body, names);
                }
            }
            _ => {}
        }
    }
}

fn base_methods(runtime: &str) -> Result<BTreeMap<String, BTreeSet<String>>, Failure> {
    let module = ast::Suite::parse(runtime, "_async.py")?;
    let mut methods = BTreeMap::new();

    for statement in &module {
        if let Stmt::ClassDef(class) = statement {
            let mut names = BTreeSet::new();
            collect_functions(&class.body, &mut names);
            methods.insert(class.name.to_string(), names);
        }
    }

    return Ok(methods);
}

fn generate(stub: &str, runtime: &str) -> Result<String, Failure> {
    let module = ast::Suite::parse(stub, "_termtrace.pyi")?;
    let source = Source { text: stub };
    let classes = module
        .iter()
        .filter_map(|statement| {
            let Stmt::ClassDef(class) = statement else {
                return None;
            };

            return Some((class.name.to_string(), class));
        })
        .collect::<BTreeMap<String, &StmtClassDef>>();

    let inherited = base_methods(runtime)?;

    let mut out = HEADER
        .iter()
        .map(|line| return (*line).to_owned())
        .collect::<Vec<_>>();

    let all = WRAPPED
        .iter()
        .map(|name| return format!("\"Async{name}\""))
        .collect::<Vec<_>>();
    out.push(format!("__all__ = [{}]", all.join(", ")));

    for class in WRAPPED {
        let node = classes
            .get(class)
            .ok_or_else(|| return format!("the stub has no class `{class}`"))?;

        let functions = node
            .body
            .iter()
            .filter_map(|statement| {
                let Stmt::FunctionDef(function) = statement else {
                    return None;
                };

                return Some(function);
            })
            .collect::<Vec<_>>();

        let new = functions
            .iter()
            .find(|function| return function.name.as_str() == "__new__");

        out.push(String::new());
        out.push(String::new());
        out.push(format!(
            "class Async{class}({}):",
            base(class).unwrap_or("Wrapper")
        ));

        if let Some(doc) = docstring_text(&node.body).filter(|doc| return !doc.is_empty()) {
            let first = doc.trim().lines().next().unwrap_or_default();
            out.push(format!("    r\"\"\"Asynchronous {class}: {first}\"\"\""));
        }

        out.push(String::new());
        out.extend(constructor(&source, class, new.copied())?);

        let excluded = match base(class) {
            Some(base) => inherited
                .get(base)
                .ok_or_else(|| return format!("_async.py has no class `{base}`"))?
                .iter()
                .filter(|name| return name.as_str() != "wait_for")
                .cloned()
                .collect::<BTreeSet<_>>(),
            None => BTreeSet::new(),
        };
        for function in functions {
            let name = function.name.as_str();
            if excluded.contains(name)
                || SKIPPED.contains(&name)
                || decorated(function, "staticmethod")
            {
                continue;
            }

            out.extend(method(&source, class, function)?);
            out.push(String::new());
        }

        while out.last().is_some_and(String::is_empty) {
            out.pop();
        }
    }

    return Ok(format!("{}\n", out.join("\n")));
}
