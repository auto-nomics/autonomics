//! Tree-walking interpreter over the [`rustpython_parser`] AST.
//!
//! The evaluation surface is closed: the only callable names are the graph API
//! registered in [`super::api`] and a small set of pure builtins implemented
//! here. Everything else — modules, classes, attributes outside the known
//! method tables — is unimplemented and fails with a structured error, which
//! is the sandbox: there is nothing to escape into. Resource limits (operation
//! budget, wall-clock deadline, call depth, container sizes, value depth) are
//! enforced at the corresponding construction sites.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use num_traits::ToPrimitive;
use rustpython_ast as ast;
use rustpython_ast::Ranged;
use rustpython_parser::{Mode, parse};

use super::api;
use super::{
    DagShellError, DagShellSnapshot, GraphEditOp, MAX_API_OPERATIONS, MAX_ARRAY_SIZE,
    MAX_CALL_LEVELS, MAX_EXPRESSION_DEPTH, MAX_INDENTATION, MAX_MAP_SIZE, MAX_SCRIPT_OPERATIONS,
    MAX_STRING_SIZE, ScriptExecution, ScriptState,
};
use crate::node_registry::registry::NodeRegistry;

/// Maximum nesting level of runtime list/dict/tuple values. Also the depth cap
/// of JSON conversion, mirroring the previous `dynamic_to_json` behavior.
const MAX_VALUE_DEPTH: usize = 64;

// ── Values ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) enum Value {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    List(Rc<RefCell<ListData>>),
    Tuple(Rc<TupleData>),
    Dict(Rc<RefCell<DictData>>),
    Range(Rc<RangeData>),
    Function(Rc<FunctionData>),
    Builtin(&'static str),
    /// Transient carrier for dict-comprehension pairs; `value_to_json`
    /// rejects it, so it can never leak into results or staged specs.
    Pair(Box<Value>, Box<Value>),
}

pub(crate) struct ListData {
    items: Vec<Value>,
    depth: usize,
}

pub(crate) struct TupleData {
    items: Vec<Value>,
    depth: usize,
}

pub(crate) struct DictData {
    entries: Vec<(DictKey, Value)>,
    index: HashMap<DictKey, usize>,
    depth: usize,
}

pub(crate) struct RangeData {
    start: i64,
    stop: i64,
    step: i64,
}

pub(crate) struct FunctionData {
    name: String,
    params: Vec<Param>,
    body: Rc<Vec<ast::Stmt>>,
    closure: ScopeRef,
}

pub(crate) struct Param {
    name: String,
    default: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DictKey {
    None,
    Bool(bool),
    Int(i64),
    Str(Rc<str>),
    Tuple(Rc<Vec<DictKey>>),
}

impl DictKey {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::None => Ok(Self::None),
            Value::Bool(b) => Ok(Self::Bool(*b)),
            Value::Int(i) => Ok(Self::Int(*i)),
            Value::Str(s) => Ok(Self::Str(Rc::clone(s))),
            Value::Tuple(t) => {
                let mut keys = Vec::with_capacity(t.items.len());
                for item in &t.items {
                    keys.push(Self::from_value(item)?);
                }
                Ok(Self::Tuple(Rc::new(keys)))
            }
            other => Err(format!("{} is unhashable as a dict key", other.type_name())),
        }
    }

    fn to_display(&self) -> String {
        match self {
            Self::None => "None".into(),
            Self::Bool(b) => if *b { "True" } else { "False" }.into(),
            Self::Int(i) => i.to_string(),
            Self::Str(s) => s.to_string(),
            Self::Tuple(keys) => {
                let inner: Vec<String> = keys.iter().map(Self::to_display).collect();
                format!("({})", inner.join(", "))
            }
        }
    }
}

pub(crate) struct Scope {
    vars: RefCell<HashMap<String, Value>>,
    parent: Option<ScopeRef>,
}

pub(crate) type ScopeRef = Rc<Scope>;

impl Scope {
    fn new(parent: Option<ScopeRef>) -> ScopeRef {
        Rc::new(Scope {
            vars: RefCell::new(HashMap::new()),
            parent,
        })
    }

    fn get(&self, name: &str) -> Option<Value> {
        let mut scope = Some(self);
        while let Some(current) = scope {
            if let Some(value) = current.vars.borrow().get(name) {
                return Some(value.clone());
            }
            scope = current.parent.as_deref();
        }
        None
    }

    fn set(&self, name: String, value: Value) {
        self.vars.borrow_mut().insert(name, value);
    }
}

// ── Control flow and errors ────────────────────────────────────────────────

enum Flow {
    Normal,
    Break,
    Continue,
    Return(Value),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrKind {
    Type,
    Value,
    Name,
    Attribute,
    Key,
    Index,
    ZeroDivision,
    Runtime,
    Limit,
    Timeout,
}

impl ErrKind {
    fn headline(self) -> &'static str {
        match self {
            Self::Type => "TypeError",
            Self::Value => "ValueError",
            Self::Name => "NameError",
            Self::Attribute => "AttributeError",
            Self::Key => "KeyError",
            Self::Index => "IndexError",
            Self::ZeroDivision => "ZeroDivisionError",
            Self::Runtime => "RuntimeError",
            Self::Limit => "LimitError",
            Self::Timeout => "TimeoutError",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct EvalError {
    pub(crate) kind: ErrKind,
    pub(crate) message: String,
    pub(crate) offset: ast::TextSize,
}

pub(crate) type EvalResult<T> = Result<T, EvalError>;

impl Value {
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Self::None => "NoneType",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "str",
            Self::List(_) => "list",
            Self::Tuple(_) => "tuple",
            Self::Dict(_) => "dict",
            Self::Range(_) => "range",
            Self::Function(_) | Self::Builtin(_) => "function",
            Self::Pair(_, _) => "object",
        }
    }

    fn depth(&self) -> usize {
        match self {
            Self::List(list) => list.borrow().depth,
            Self::Tuple(tuple) => tuple.depth,
            Self::Dict(dict) => dict.borrow().depth,
            Self::Pair(key, value) => key.depth().max(value.depth()),
            _ => 0,
        }
    }

    fn len(&self) -> Option<usize> {
        match self {
            Self::Str(s) => Some(s.chars().count()),
            Self::List(list) => Some(list.borrow().items.len()),
            Self::Tuple(tuple) => Some(tuple.items.len()),
            Self::Dict(dict) => Some(dict.borrow().entries.len()),
            Self::Range(range) => Some(range_len(range)),
            _ => None,
        }
    }
}

fn range_len(range: &RangeData) -> usize {
    let diff = i128::from(range.stop) - i128::from(range.start);
    let step = i128::from(range.step);
    let raw = if step > 0 {
        if diff <= 0 {
            0
        } else {
            (diff + step - 1).div_euclid(step)
        }
    } else if diff >= 0 {
        0
    } else {
        (diff + step + 1).div_euclid(step)
    };
    raw.max(0) as usize
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::None => false,
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::List(list) => !list.borrow().items.is_empty(),
        Value::Tuple(tuple) => !tuple.items.is_empty(),
        Value::Dict(dict) => !dict.borrow().entries.is_empty(),
        Value::Range(range) => range_len(range) > 0,
        Value::Function(_) | Value::Builtin(_) | Value::Pair(_, _) => true,
    }
}

// ── Source prescan (runs before parsing, on the caller thread) ─────────────

/// Strip a shared leading-whitespace prefix from every non-blank line
/// (textwrap.dedent semantics). Keeps multi-line, tool-call-indented scripts
/// parseable; relative indentation is preserved.
fn dedent(script: &str) -> String {
    let mut prefix: Option<String> = None;
    for line in script.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indent: String = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        prefix = Some(match prefix {
            None => indent,
            Some(current) => current
                .chars()
                .zip(indent.chars())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect(),
        });
        if prefix.as_deref() == Some("") {
            return script.to_string();
        }
    }
    let Some(prefix) = prefix.filter(|p| !p.is_empty()) else {
        return script.to_string();
    };
    let mut out = String::with_capacity(script.len());
    for line in script.lines() {
        out.push_str(line.strip_prefix(prefix.as_str()).unwrap_or(line));
        out.push('\n');
    }
    out
}

/// Lexical pre-scan guarding the recursive-descent parser and the recursive
/// validator/evaluator: bracket nesting depth and top-level statement
/// indentation. Stack overflow in Rust aborts the process and cannot be
/// caught, so this must run before any recursion happens.
pub(crate) fn prescan_source(source: &str) -> Result<(), DagShellError> {
    let bytes = source.as_bytes();
    let mut depth: usize = 0;
    let mut max_depth: usize = 0;
    let mut max_indent: usize = 0;
    let mut i = 0usize;
    let mut quote: Option<(u8, u32)> = None; // (delimiter, consecutive delimiter run)
    let mut escaped = false;
    let mut line_start = true;
    let mut indent_columns = 0usize;

    while i < bytes.len() {
        let byte = bytes[i];
        if let Some((delimiter, run)) = quote {
            if escaped {
                escaped = false;
                i += 1;
                continue;
            }
            match byte {
                b'\\' => escaped = true,
                c if c == delimiter => {
                    let run = run + 1;
                    if run == 3 {
                        quote = None;
                    } else {
                        quote = Some((delimiter, run));
                    }
                }
                b'\n' if delimiter != b'"' && delimiter != b'\'' => quote = Some((delimiter, 0)),
                _ => quote = Some((delimiter, 0)),
            }
            i += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => {
                quote = Some((byte, 1));
                line_start = false;
            }
            b'\\' => {
                escaped = true;
                line_start = false;
            }
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                max_depth = max_depth.max(depth);
                line_start = false;
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                line_start = false;
            }
            b'\n' => {
                if depth == 0 {
                    max_indent = max_indent.max(indent_columns);
                }
                indent_columns = 0;
                line_start = true;
            }
            b' ' if line_start => indent_columns += 1,
            b'\t' if line_start => indent_columns += 8,
            _ => line_start = false,
        }
        i += 1;
    }

    if max_depth > MAX_EXPRESSION_DEPTH {
        return Err(DagShellError {
            code: "parse_error".into(),
            message: format!("expression nesting too deep: maximum {MAX_EXPRESSION_DEPTH} levels"),
            operation_index: None,
            line: None,
            column: None,
        });
    }
    if max_indent > MAX_INDENTATION {
        return Err(DagShellError {
            code: "parse_error".into(),
            message: format!("statement nesting too deep: maximum {MAX_INDENTATION} columns"),
            operation_index: None,
            line: None,
            column: None,
        });
    }
    Ok(())
}

// ── Static validation (statement whitelist, runs before evaluation) ────────

/// Rejects every statement kind outside the sandbox surface *before*
/// evaluation, so unreachable code cannot hide disallowed constructs.
fn validate_suite(suite: &[ast::Stmt], in_function: bool) -> Result<(), (String, ast::TextSize)> {
    for stmt in suite {
        match stmt {
            ast::Stmt::FunctionDef(def) => {
                if !def.decorator_list.is_empty() {
                    return Err(("function decorators are not supported".into(), def.start()));
                }
                let args = &def.args;
                if args.vararg.is_some() || args.kwarg.is_some() {
                    return Err(("*args/**kwargs are not supported".into(), def.start()));
                }
                if !args.kwonlyargs.is_empty() {
                    return Err((
                        "keyword-only arguments are not supported".into(),
                        def.start(),
                    ));
                }
                validate_suite(&def.body, true)?;
            }
            ast::Stmt::Return(ret) if !in_function => {
                return Err(("'return' outside function".into(), ret.start()));
            }
            ast::Stmt::If(if_stmt) => {
                validate_suite(&if_stmt.body, in_function)?;
                validate_suite(&if_stmt.orelse, in_function)?;
            }
            ast::Stmt::For(for_stmt) => {
                validate_suite(&for_stmt.body, in_function)?;
                validate_suite(&for_stmt.orelse, in_function)?;
            }
            ast::Stmt::While(while_stmt) => {
                validate_suite(&while_stmt.body, in_function)?;
                validate_suite(&while_stmt.orelse, in_function)?;
            }
            ast::Stmt::Return(_)
            | ast::Stmt::Assign(_)
            | ast::Stmt::AugAssign(_)
            | ast::Stmt::Expr(_)
            | ast::Stmt::Pass(_)
            | ast::Stmt::Break(_)
            | ast::Stmt::Continue(_) => {}
            ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_) => {
                return Err((
                    "import is disabled: dag_shell scripts can only use the registered graph API"
                        .into(),
                    stmt.start(),
                ));
            }
            other => {
                return Err((
                    format!(
                        "{} is not supported in dag_shell scripts",
                        other_headline(other)
                    ),
                    other.start(),
                ));
            }
        }
    }
    Ok(())
}

fn other_headline(stmt: &ast::Stmt) -> String {
    let name = match stmt {
        ast::Stmt::ClassDef(_) => "'class' definitions",
        ast::Stmt::Try(_) | ast::Stmt::TryStar(_) => "'try/except'",
        ast::Stmt::With(_) => "'with' blocks",
        ast::Stmt::Raise(_) => "'raise'",
        ast::Stmt::Assert(_) => "'assert'",
        ast::Stmt::Global(_) | ast::Stmt::Nonlocal(_) => "'global'/'nonlocal'",
        ast::Stmt::Delete(_) => "'del'",
        ast::Stmt::AsyncFunctionDef(_) | ast::Stmt::AsyncFor(_) | ast::Stmt::AsyncWith(_) => {
            "'async' statements"
        }
        ast::Stmt::Match(_) => "'match' statements",
        ast::Stmt::AnnAssign(_) => "annotated assignment",
        ast::Stmt::TypeAlias(_) => "type aliases",
        _ => "this statement",
    };
    name.to_string()
}

// ── Interpreter ────────────────────────────────────────────────────────────

struct Interp<'a> {
    state: Arc<Mutex<ScriptState>>,
    snapshot: &'a DagShellSnapshot,
    registry: &'a Arc<NodeRegistry>,
    ops: u64,
    deadline: Instant,
    depth: usize,
}

pub(crate) fn run(
    raw_script: &str,
    started: Instant,
    timeout: std::time::Duration,
    snapshot: &DagShellSnapshot,
    registry: &Arc<NodeRegistry>,
) -> Result<ScriptExecution, DagShellError> {
    // Agents routinely paste script bodies with a shared base indentation;
    // strip a common whitespace prefix so Python's indentation rules apply to
    // the script's own structure, not the tool-call formatting.
    let script = dedent(raw_script);
    let located = |offset: ast::TextSize| -> (Option<u32>, Option<u32>) {
        let mut locator = ast::source_code::RandomLocator::new(&script);
        let location = locator.locate(offset);
        (Some(location.row.get()), Some(location.column.get()))
    };

    let module = match parse(&script, Mode::Module, "<dag_shell>") {
        Ok(module) => module,
        Err(parse_error) => {
            let (line, column) = located(parse_error.offset);
            return Err(DagShellError {
                code: "parse_error".into(),
                message: format!("SyntaxError: {}", parse_error.error),
                operation_index: None,
                line,
                column,
            });
        }
    };
    let ast::Mod::Module(module) = module else {
        return Err(DagShellError {
            code: "parse_error".into(),
            message: "script must be a module".into(),
            operation_index: None,
            line: None,
            column: None,
        });
    };

    if let Err((message, offset)) = validate_suite(&module.body, false) {
        let (line, column) = located(offset);
        return Err(DagShellError {
            code: "parse_error".into(),
            message,
            operation_index: None,
            line,
            column,
        });
    }

    let mut interp = Interp {
        state: Arc::new(Mutex::new(ScriptState::default())),
        snapshot,
        registry,
        ops: 0,
        deadline: started + timeout,
        depth: 0,
    };
    let globals = Scope::new(None);

    // Execute; the result is the value of the final expression statement.
    let executed: EvalResult<Value> = (|| {
        let mut result = Value::None;
        for stmt in &module.body {
            if let ast::Stmt::Expr(expr_stmt) = stmt {
                result = interp.eval_expr(&expr_stmt.value, &globals)?;
                continue;
            }
            match interp.exec_stmt(stmt, &globals)? {
                Flow::Normal | Flow::Continue => {}
                Flow::Break => {
                    return Err(interp.error(
                        ErrKind::Runtime,
                        "'break' outside loop",
                        stmt.start(),
                    ));
                }
                Flow::Return(value) => {
                    // Module-level `return` is rejected during validation;
                    // this is defense in depth for unvalidated paths.
                    result = value;
                    break;
                }
            }
        }
        Ok(result)
    })();
    let result = match executed {
        Ok(result) => result,
        Err(eval_error) => return Err(eval_error_to_dag_shell(&script, eval_error)),
    };

    let (operations, committed) = {
        let state = interp.state.lock().map_err(|_| DagShellError {
            code: "runtime_error".into(),
            message: "script state poisoned".into(),
            operation_index: None,
            line: None,
            column: None,
        })?;
        (state.operations.clone(), state.committed)
    };
    let serialized = serialize_result(&result)?;

    Ok(ScriptExecution {
        operations,
        committed,
        result: serialized,
    })
}

fn eval_error_to_dag_shell(script: &str, error: EvalError) -> DagShellError {
    let mut locator = ast::source_code::RandomLocator::new(script);
    let location = locator.locate(error.offset);
    let (code, message) = match error.kind {
        ErrKind::Timeout => ("timeout", "script timed out".to_string()),
        ErrKind::Limit => ("limit_exceeded", error.message),
        other => (
            "runtime_error",
            format!("{}: {}", other.headline(), error.message),
        ),
    };
    DagShellError {
        code: code.into(),
        message,
        operation_index: None,
        line: Some(location.row.get()),
        column: Some(location.column.get()),
    }
}

fn serialize_result(result: &Value) -> Result<String, DagShellError> {
    let json = value_to_json(result, 0).ok_or_else(|| DagShellError {
        code: "limit_exceeded".into(),
        message: "script result contains an unsupported or oversized value".into(),
        operation_index: None,
        line: None,
        column: None,
    })?;
    let serialized = serde_json::to_string(&json).map_err(|serde_error| DagShellError {
        code: "runtime_error".into(),
        message: format!("cannot serialize script result: {serde_error}"),
        operation_index: None,
        line: None,
        column: None,
    })?;
    if serialized.len() > super::MAX_RESULT_BYTES {
        return Err(DagShellError {
            code: "limit_exceeded".into(),
            message: format!("script result exceeds {} bytes", super::MAX_RESULT_BYTES),
            operation_index: None,
            line: None,
            column: None,
        });
    }
    Ok(serialized)
}

impl<'a> Interp<'a> {
    fn error(&self, kind: ErrKind, message: impl Into<String>, offset: ast::TextSize) -> EvalError {
        EvalError {
            kind,
            message: message.into(),
            offset,
        }
    }

    fn type_error(&self, offset: ast::TextSize, message: impl Into<String>) -> EvalError {
        self.error(ErrKind::Type, message, offset)
    }

    fn limit_error(&self, offset: ast::TextSize, message: impl Into<String>) -> EvalError {
        self.error(ErrKind::Limit, message, offset)
    }

    fn value_error(&self, offset: ast::TextSize, message: impl Into<String>) -> EvalError {
        self.error(ErrKind::Value, message, offset)
    }

    fn unimplemented(&self, what: &str, offset: ast::TextSize) -> EvalError {
        self.error(
            ErrKind::Runtime,
            format!("{what} are not supported in dag_shell scripts"),
            offset,
        )
    }

    fn charge(&mut self, offset: ast::TextSize) -> EvalResult<()> {
        self.ops += 1;
        if self.ops > MAX_SCRIPT_OPERATIONS {
            return Err(self.limit_error(offset, "operation budget exceeded"));
        }
        if Instant::now() >= self.deadline {
            return Err(self.error(ErrKind::Timeout, "script timed out", offset));
        }
        Ok(())
    }

    fn exec_block(&mut self, suite: &[ast::Stmt], scope: &ScopeRef) -> EvalResult<Flow> {
        for stmt in suite {
            match self.exec_stmt(stmt, scope)? {
                Flow::Normal => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_stmt(&mut self, stmt: &ast::Stmt, scope: &ScopeRef) -> EvalResult<Flow> {
        self.charge(stmt.start())?;
        match stmt {
            ast::Stmt::Expr(expr_stmt) => {
                self.eval_expr(&expr_stmt.value, scope)?;
                Ok(Flow::Normal)
            }
            ast::Stmt::Assign(assign) => {
                let value = self.eval_expr(&assign.value, scope)?;
                for target in &assign.targets {
                    self.assign_to(target, value.clone(), scope)?;
                }
                Ok(Flow::Normal)
            }
            ast::Stmt::AugAssign(aug) => {
                let rhs = self.eval_expr(&aug.value, scope)?;
                let current = self.eval_expr(&aug.target, scope)?;
                let combined = self.bin_op(aug.op, current, rhs, aug.target.start())?;
                self.assign_to(&aug.target, combined, scope)?;
                Ok(Flow::Normal)
            }
            ast::Stmt::If(if_stmt) => {
                let test = self.eval_expr(&if_stmt.test, scope)?;
                if truthy(&test) {
                    self.exec_block(&if_stmt.body, scope)
                } else {
                    self.exec_block(&if_stmt.orelse, scope)
                }
            }
            ast::Stmt::While(while_stmt) => {
                let mut ran = false;
                loop {
                    self.charge(while_stmt.start())?;
                    let test = self.eval_expr(&while_stmt.test, scope)?;
                    if !truthy(&test) {
                        break;
                    }
                    ran = true;
                    match self.exec_block(&while_stmt.body, scope)? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => return Ok(Flow::Normal),
                        flow @ Flow::Return(_) => return Ok(flow),
                    }
                }
                let _ = ran;
                self.exec_block(&while_stmt.orelse, scope)
            }
            ast::Stmt::For(for_stmt) => {
                let iterable = self.eval_expr(&for_stmt.iter, scope)?;
                let mut iterant = self.make_iterant(&iterable, for_stmt.iter.start())?;
                loop {
                    self.charge(for_stmt.start())?;
                    let Some(item) = iterant.next() else {
                        break;
                    };
                    self.bind_target(&for_stmt.target, item, scope)?;
                    match self.exec_block(&for_stmt.body, scope)? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => return Ok(Flow::Normal),
                        flow @ Flow::Return(_) => return Ok(flow),
                    }
                }
                self.exec_block(&for_stmt.orelse, scope)
            }
            ast::Stmt::FunctionDef(def) => {
                let mut params = Vec::new();
                for arg in def.args.posonlyargs.iter().chain(def.args.args.iter()) {
                    let default = match &arg.default {
                        Some(expr) => Some(self.eval_expr(expr, scope)?),
                        None => None,
                    };
                    params.push(Param {
                        name: arg.def.arg.as_str().to_string(),
                        default,
                    });
                }
                let value = Value::Function(Rc::new(FunctionData {
                    name: def.name.as_str().to_string(),
                    params,
                    body: Rc::new(def.body.clone()),
                    closure: Rc::clone(scope),
                }));
                scope.set(def.name.as_str().to_string(), value);
                Ok(Flow::Normal)
            }
            ast::Stmt::Return(ret) => {
                let value = match &ret.value {
                    Some(value) => self.eval_expr(value, scope)?,
                    None => Value::None,
                };
                Ok(Flow::Return(value))
            }
            ast::Stmt::Pass(_) => Ok(Flow::Normal),
            ast::Stmt::Break(_) => Ok(Flow::Break),
            ast::Stmt::Continue(_) => Ok(Flow::Continue),
            // Everything else was rejected by `validate_suite` before
            // evaluation; this arm is defense in depth.
            other => Err(self.error(
                ErrKind::Runtime,
                format!(
                    "{} is not supported in dag_shell scripts",
                    other_headline(other)
                ),
                other.start(),
            )),
        }
    }

    fn assign_to(&mut self, target: &ast::Expr, value: Value, scope: &ScopeRef) -> EvalResult<()> {
        match target {
            ast::Expr::Name(name) => {
                scope.set(name.id.as_str().to_string(), value);
                Ok(())
            }
            ast::Expr::Tuple(tuple) => self.unpack(&tuple.elts, value, tuple.start(), scope),
            ast::Expr::List(list) => self.unpack(&list.elts, value, list.start(), scope),
            ast::Expr::Subscript(subscript) => self.store_subscript(subscript, value, scope),
            _ => Err(self.type_error(
                target.start(),
                "invalid assignment target: use a name, tuple pattern, or subscript",
            )),
        }
    }

    fn unpack(
        &mut self,
        targets: &[ast::Expr],
        value: Value,
        offset: ast::TextSize,
        scope: &ScopeRef,
    ) -> EvalResult<()> {
        for element in targets {
            if let ast::Expr::Starred(_) = element {
                return Err(self.unimplemented("starred assignment targets", element.start()));
            }
        }
        let items: Vec<Value> = match value {
            Value::List(list) => list.borrow().items.clone(),
            Value::Tuple(tuple) => tuple.items.clone(),
            other => {
                return Err(self.type_error(offset, format!("cannot unpack {}", other.type_name())));
            }
        };
        if items.len() != targets.len() {
            return Err(self.value_error(
                offset,
                format!(
                    "unpacking expects {} values, got {}",
                    targets.len(),
                    items.len()
                ),
            ));
        }
        for (target, item) in targets.iter().zip(items) {
            self.assign_to(target, item, scope)?;
        }
        Ok(())
    }

    fn store_subscript(
        &mut self,
        subscript: &ast::ExprSubscript,
        value: Value,
        scope: &ScopeRef,
    ) -> EvalResult<()> {
        let receiver = self.eval_expr(&subscript.value, scope)?;
        let index_expr = &subscript.slice;
        match &receiver {
            Value::Dict(dict) => {
                let key_value = self.eval_expr(index_expr, scope)?;
                let key = self.dict_key(&key_value, index_expr.start())?;
                self.check_value_depth(&value, index_expr.start())?;
                let mut dict = dict.borrow_mut();
                dict_insert(&mut dict, key, value);
                Ok(())
            }
            Value::List(list) => {
                let index_value = self.eval_expr(index_expr, scope)?;
                let index = self.int_index(&index_value, index_expr.start())?;
                self.check_value_depth(&value, index_expr.start())?;
                let mut list = list.borrow_mut();
                let resolved = resolve_index(index, list.items.len())
                    .map_err(|error| self.error(ErrKind::Index, error, index_expr.start()))?;
                list.items[resolved] = value;
                Ok(())
            }
            other => Err(self.type_error(
                subscript.start(),
                format!(
                    "'{}' object does not support item assignment",
                    other.type_name()
                ),
            )),
        }
    }

    // ── Expressions ────────────────────────────────────────────────────────

    fn eval_expr(&mut self, expr: &ast::Expr, scope: &ScopeRef) -> EvalResult<Value> {
        self.charge(expr.start())?;
        match expr {
            ast::Expr::Constant(constant) => self.constant(&constant.value, expr.start()),
            ast::Expr::Name(name) => self.lookup(name.id.as_str(), scope, name.start()),
            ast::Expr::List(list) => {
                let mut items = Vec::with_capacity(list.elts.len());
                for element in &list.elts {
                    if let ast::Expr::Starred(_) = element {
                        return Err(self.unimplemented("starred expressions", element.start()));
                    }
                    items.push(self.eval_expr(element, scope)?);
                }
                self.make_list(items, list.start())
            }
            ast::Expr::Tuple(tuple) => {
                let mut items = Vec::with_capacity(tuple.elts.len());
                for element in &tuple.elts {
                    if let ast::Expr::Starred(_) = element {
                        return Err(self.unimplemented("starred expressions", element.start()));
                    }
                    items.push(self.eval_expr(element, scope)?);
                }
                self.make_tuple(items, tuple.start())
            }
            ast::Expr::Dict(dict_expr) => {
                let Value::Dict(dict_cell) = self.make_dict(Vec::new(), dict_expr.start())? else {
                    unreachable!("make_dict returns Dict");
                };
                for (key_expr, value_expr) in dict_expr.keys.iter().zip(&dict_expr.values) {
                    let Some(key_expr) = key_expr else {
                        return Err(self.unimplemented("dict unpacking (**)", dict_expr.start()));
                    };
                    let key_value = self.eval_expr(key_expr, scope)?;
                    let key = self.dict_key(&key_value, key_expr.start())?;
                    let value = self.eval_expr(value_expr, scope)?;
                    self.check_value_depth(&value, key_expr.start())?;
                    let mut dict = dict_cell.borrow_mut();
                    if dict.entries.len() >= MAX_MAP_SIZE {
                        return Err(self.limit_error(dict_expr.start(), "map size limit exceeded"));
                    }
                    dict_insert(&mut dict, key, value);
                }
                Ok(Value::Dict(dict_cell))
            }
            ast::Expr::BinOp(bin_op) => {
                let left = self.eval_expr(&bin_op.left, scope)?;
                let right = self.eval_expr(&bin_op.right, scope)?;
                self.bin_op(bin_op.op, left, right, bin_op.start())
            }
            ast::Expr::UnaryOp(unary) => {
                let operand = self.eval_expr(&unary.operand, scope)?;
                match unary.op {
                    ast::UnaryOp::Not => Ok(Value::Bool(!truthy(&operand))),
                    ast::UnaryOp::USub => match operand {
                        Value::Int(value) => value
                            .checked_neg()
                            .map(Value::Int)
                            .ok_or_else(|| self.limit_error(unary.start(), "integer overflow")),
                        Value::Float(value) => Ok(Value::Float(-value)),
                        other => Err(self.type_error(
                            unary.start(),
                            format!("bad operand type for unary -: '{}'", other.type_name()),
                        )),
                    },
                    ast::UnaryOp::UAdd => match operand {
                        Value::Int(_) | Value::Float(_) => Ok(operand),
                        other => Err(self.type_error(
                            unary.start(),
                            format!("bad operand type for unary +: '{}'", other.type_name()),
                        )),
                    },
                    ast::UnaryOp::Invert => {
                        Err(self.unimplemented("the '~' operator", unary.start()))
                    }
                }
            }
            ast::Expr::BoolOp(bool_op) => {
                let mut last = Value::None;
                for value_expr in &bool_op.values {
                    let value = self.eval_expr(value_expr, scope)?;
                    match bool_op.op {
                        ast::BoolOp::And if !truthy(&value) => return Ok(value),
                        ast::BoolOp::Or if truthy(&value) => return Ok(value),
                        _ => {}
                    }
                    last = value;
                }
                Ok(last)
            }
            ast::Expr::Compare(compare) => {
                let mut left = self.eval_expr(&compare.left, scope)?;
                for (op, comparator) in compare.ops.iter().zip(&compare.comparators) {
                    let right = self.eval_expr(comparator, scope)?;
                    if !self.compare(op, &left, &right, comparator.start())? {
                        return Ok(Value::Bool(false));
                    }
                    left = right;
                }
                Ok(Value::Bool(true))
            }
            ast::Expr::IfExp(if_exp) => {
                let test = self.eval_expr(&if_exp.test, scope)?;
                if truthy(&test) {
                    self.eval_expr(&if_exp.body, scope)
                } else {
                    self.eval_expr(&if_exp.orelse, scope)
                }
            }
            ast::Expr::Subscript(subscript) => self.subscript_read(subscript, scope),
            ast::Expr::Attribute(attribute) => {
                let receiver = self.eval_expr(&attribute.value, scope)?;
                Err(self.attribute_error(&receiver, attribute.attr.as_str(), attribute.start()))
            }
            ast::Expr::Call(call) => self.eval_call(call, scope),
            ast::Expr::JoinedStr(joined) => {
                let mut out = String::new();
                for part in &joined.values {
                    let text = self.fstring_part(part, scope)?;
                    out.push_str(&text);
                }
                self.make_string(out, joined.start())
            }
            ast::Expr::FormattedValue(formatted) => {
                let text = self.fstring_part(expr, scope)?;
                let _ = formatted;
                self.make_string(text.to_string(), formatted.start())
            }
            ast::Expr::ListComp(comp) => {
                let mut items = Vec::new();
                let scope = Scope::new(Some(Rc::clone(scope)));
                self.comp_emit(
                    &comp.generators,
                    0,
                    &scope,
                    &mut items,
                    &mut |interp, scope| interp.eval_expr(&comp.elt, scope),
                )?;
                self.make_list(items, comp.start())
            }
            ast::Expr::DictComp(comp) => {
                let Value::Dict(dict_cell) = self.make_dict(Vec::new(), comp.start())? else {
                    unreachable!("make_dict returns Dict");
                };
                let comp_scope = Scope::new(Some(Rc::clone(scope)));
                let mut pairs = Vec::new();
                self.comp_emit(
                    &comp.generators,
                    0,
                    &comp_scope,
                    &mut pairs,
                    &mut |interp, scope| {
                        let key = interp.eval_expr(&comp.key, scope)?;
                        let value = interp.eval_expr(&comp.value, scope)?;
                        Ok(Value::Pair(Box::new(key), Box::new(value)))
                    },
                )?;
                for pair in pairs {
                    let Value::Pair(key_value, value) = pair else {
                        unreachable!("dict comprehension emitter returns pairs");
                    };
                    let key = self.dict_key(&key_value, comp.key.start())?;
                    let value = *value;
                    self.check_value_depth(&value, comp.start())?;
                    let mut dict = dict_cell.borrow_mut();
                    if dict.entries.len() >= MAX_MAP_SIZE {
                        return Err(self.limit_error(comp.start(), "map size limit exceeded"));
                    }
                    dict_insert(&mut dict, key, value);
                }
                Ok(Value::Dict(dict_cell))
            }
            ast::Expr::GeneratorExp(generator) => {
                // Materialized like a list comprehension; laziness is not
                // observable at this scale and keeps every consumer capped.
                let mut items = Vec::new();
                let scope = Scope::new(Some(Rc::clone(scope)));
                self.comp_emit(
                    &generator.generators,
                    0,
                    &scope,
                    &mut items,
                    &mut |interp, scope| interp.eval_expr(&generator.elt, scope),
                )?;
                self.make_list(items, generator.start())
            }
            ast::Expr::NamedExpr(named) => {
                let value = self.eval_expr(&named.value, scope)?;
                self.assign_to(&named.target, value.clone(), scope)?;
                Ok(value)
            }
            other => {
                let name = match other {
                    ast::Expr::Lambda(_) => "lambda expressions",
                    ast::Expr::Set(_) | ast::Expr::SetComp(_) => "set literals",
                    ast::Expr::Starred(_) => "starred expressions",
                    ast::Expr::Await(_) => "'await' expressions",
                    ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) => "generator expressions",
                    ast::Expr::Slice(_) => "bare slice expressions",
                    _ => "this expression",
                };
                Err(self.unimplemented(name, other.start()))
            }
        }
    }

    fn fstring_part(&mut self, part: &ast::Expr, scope: &ScopeRef) -> EvalResult<Rc<str>> {
        match part {
            ast::Expr::Constant(constant) => match &constant.value {
                ast::Constant::Str(text) => Ok(Rc::from(text.as_str())),
                _ => Err(self.type_error(part.start(), "invalid f-string literal part")),
            },
            ast::Expr::FormattedValue(formatted) => {
                if formatted.format_spec.is_some() {
                    return Err(self.unimplemented("f-string format specs", part.start()));
                }
                let value = self.eval_expr(&formatted.value, scope)?;
                let text = match formatted.conversion {
                    ast::ConversionFlag::None | ast::ConversionFlag::Str => self.to_display(&value),
                    ast::ConversionFlag::Repr | ast::ConversionFlag::Ascii => self.repr(&value),
                };
                Ok(Rc::from(text.as_str()))
            }
            other => Err(self.type_error(other.start(), "invalid f-string part")),
        }
    }

    fn constant(&self, constant: &ast::Constant, offset: ast::TextSize) -> EvalResult<Value> {
        match constant {
            ast::Constant::None => Ok(Value::None),
            ast::Constant::Bool(value) => Ok(Value::Bool(*value)),
            ast::Constant::Int(value) => value
                .to_i64()
                .map(Value::Int)
                .ok_or_else(|| self.limit_error(offset, "integer literal too large")),
            ast::Constant::Float(value) => Ok(Value::Float(*value)),
            ast::Constant::Str(value) => Ok(Value::Str(Rc::from(value.as_str()))),
            ast::Constant::Tuple(values) => {
                let mut items = Vec::with_capacity(values.len());
                for value in values {
                    items.push(self.constant(value, offset)?);
                }
                self.make_tuple(items, offset)
            }
            ast::Constant::Bytes(_) => Err(self.unimplemented("bytes literals", offset)),
            ast::Constant::Complex { .. } => Err(self.unimplemented("complex numbers", offset)),
            ast::Constant::Ellipsis => Err(self.unimplemented("the ellipsis literal", offset)),
        }
    }

    fn lookup(&mut self, name: &str, scope: &ScopeRef, offset: ast::TextSize) -> EvalResult<Value> {
        if let Some(value) = scope.get(name) {
            return Ok(value);
        }
        if let Some(builtin) = builtin_function(name) {
            return Ok(Value::Builtin(builtin));
        }
        Err(self.error(
            ErrKind::Name,
            format!("name '{name}' is not defined"),
            offset,
        ))
    }

    fn eval_call(&mut self, call: &ast::ExprCall, scope: &ScopeRef) -> EvalResult<Value> {
        // Attribute receivers evaluate before arguments, matching Python's
        // evaluation order; this keeps staged-operation ordering intuitive.
        if let ast::Expr::Attribute(attribute) = &*call.func {
            let receiver = self.eval_expr(&attribute.value, scope)?;
            let args = self.eval_args(&call.args, scope)?;
            let kwargs = self.eval_kwargs(&call.keywords, scope)?;
            return self.call_method(
                receiver,
                attribute.attr.as_str(),
                &args,
                &kwargs,
                attribute.start(),
            );
        }
        let args = self.eval_args(&call.args, scope)?;
        let kwargs = self.eval_kwargs(&call.keywords, scope)?;
        let callee = self.eval_expr(&call.func, scope)?;
        match callee {
            Value::Function(function) => {
                self.call_function(&function, &args, &kwargs, call.start())
            }
            Value::Builtin(name) => {
                if api::is_host_function(name) {
                    self.call_host(name, &args, &kwargs, call.start())
                } else {
                    self.call_builtin(name, &args, &kwargs, call.start())
                }
            }
            other => Err(self.type_error(
                call.start(),
                format!("'{}' object is not callable", other.type_name()),
            )),
        }
    }

    fn eval_args(&mut self, args: &[ast::Expr], scope: &ScopeRef) -> EvalResult<Vec<Value>> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            if let ast::Expr::Starred(_) = arg {
                return Err(self.unimplemented("starred arguments (*)", arg.start()));
            }
            values.push(self.eval_expr(arg, scope)?);
        }
        Ok(values)
    }

    fn eval_kwargs(
        &mut self,
        keywords: &[ast::Keyword],
        scope: &ScopeRef,
    ) -> EvalResult<Vec<(String, Value)>> {
        let mut kwargs = Vec::with_capacity(keywords.len());
        for keyword in keywords {
            let Some(name) = &keyword.arg else {
                return Err(self.unimplemented("keyword unpacking (**)", keyword.start()));
            };
            let value = self.eval_expr(&keyword.value, scope)?;
            kwargs.push((name.as_str().to_string(), value));
        }
        Ok(kwargs)
    }

    fn call_function(
        &mut self,
        function: &Rc<FunctionData>,
        args: &[Value],
        kwargs: &[(String, Value)],
        offset: ast::TextSize,
    ) -> EvalResult<Value> {
        self.depth += 1;
        if self.depth > MAX_CALL_LEVELS {
            self.depth -= 1;
            return Err(self.limit_error(offset, "maximum recursion depth exceeded"));
        }
        let mut bindings: Vec<Option<Value>> = vec![None; function.params.len()];
        for (index, value) in args.iter().enumerate() {
            let Some(slot) = bindings.get_mut(index) else {
                self.depth -= 1;
                return Err(self.type_error(
                    offset,
                    format!(
                        "{}() takes {} positional arguments but {} were given",
                        function.name,
                        function.params.len(),
                        args.len()
                    ),
                ));
            };
            *slot = Some(value.clone());
        }
        for (name, value) in kwargs {
            let Some(position) = function.params.iter().position(|param| param.name == *name)
            else {
                self.depth -= 1;
                return Err(
                    self.type_error(offset, format!("unexpected keyword argument '{name}'"))
                );
            };
            if bindings[position].is_some() {
                self.depth -= 1;
                return Err(
                    self.type_error(offset, format!("multiple values for argument '{name}'"))
                );
            }
            bindings[position] = Some(value.clone());
        }
        for (position, slot) in bindings.iter_mut().enumerate() {
            if slot.is_none() {
                match &function.params[position].default {
                    Some(default) => *slot = Some(default.clone()),
                    None => {
                        self.depth -= 1;
                        return Err(self.type_error(
                            offset,
                            format!(
                                "missing required argument '{}'",
                                function.params[position].name
                            ),
                        ));
                    }
                }
            }
        }
        let scope = Scope::new(Some(Rc::clone(&function.closure)));
        for (param, slot) in function.params.iter().zip(bindings) {
            let value = slot.expect("all slots filled above");
            scope.set(param.name.clone(), value);
        }
        let outcome = self.exec_block(&function.body, &scope);
        self.depth -= 1;
        match outcome? {
            Flow::Return(value) => Ok(value),
            Flow::Break => Err(self.error(ErrKind::Runtime, "'break' outside loop", offset)),
            Flow::Continue => Err(self.error(ErrKind::Runtime, "'continue' outside loop", offset)),
            Flow::Normal => Ok(Value::None),
        }
    }

    // ── Host graph API ─────────────────────────────────────────────────────

    fn call_host(
        &self,
        name: &'static str,
        args: &[Value],
        kwargs: &[(String, Value)],
        offset: ast::TextSize,
    ) -> EvalResult<Value> {
        let arity = |min: usize, max: usize| -> EvalResult<()> {
            if args.len() < min || args.len() > max {
                return Err(self.type_error(
                    offset,
                    format!("{name}() takes {min}..={max} arguments, got {}", args.len()),
                ));
            }
            Ok(())
        };
        let no_kwargs = || -> EvalResult<()> {
            if !kwargs.is_empty() {
                return Err(self.type_error(offset, format!("{name}() takes no keyword arguments")));
            }
            Ok(())
        };
        match name {
            "node" => {
                arity(3, 3)?;
                no_kwargs()?;
                let id = api::str_arg(&args[0], offset)?;
                let kind = api::str_arg(&args[1], offset)?;
                let spec = api::spec_to_json(&args[2], offset)?;
                api::stage(&self.state, GraphEditOp::AddNode { id, kind, spec }, offset)?;
                Ok(Value::None)
            }
            "update_node" => {
                arity(2, 2)?;
                no_kwargs()?;
                let id = api::str_arg(&args[0], offset)?;
                let spec = api::spec_to_json(&args[1], offset)?;
                api::stage(&self.state, GraphEditOp::UpdateNode { id, spec }, offset)?;
                Ok(Value::None)
            }
            "remove_node" => {
                arity(1, 2)?;
                let mut cascade = false;
                if let Some((keyword, value)) = kwargs.first() {
                    if kwargs.len() > 1 || keyword != "cascade" {
                        return Err(self.type_error(
                            offset,
                            "remove_node() supports only the 'cascade' keyword argument",
                        ));
                    }
                    let Value::Bool(flag) = value else {
                        return Err(self.type_error(offset, "remove_node() cascade must be a bool"));
                    };
                    cascade = *flag;
                }
                let id = api::str_arg(&args[0], offset)?;
                if args.len() == 2 {
                    let Value::Bool(flag) = &args[1] else {
                        return Err(self.type_error(
                            offset,
                            "remove_node() positional cascade must be a bool",
                        ));
                    };
                    cascade = *flag;
                }
                api::stage(&self.state, GraphEditOp::RemoveNode { id, cascade }, offset)?;
                Ok(Value::None)
            }
            "edge" | "remove_edge" => {
                arity(4, 4)?;
                no_kwargs()?;
                let from = api::str_arg(&args[0], offset)?;
                let from_port = api::port_arg(&args[1], "from_port", offset)?;
                let to = api::str_arg(&args[2], offset)?;
                let to_port = api::port_arg(&args[3], "to_port", offset)?;
                let operation = if name == "edge" {
                    GraphEditOp::AddEdge {
                        from,
                        from_port,
                        to,
                        to_port,
                    }
                } else {
                    GraphEditOp::RemoveEdge {
                        from,
                        from_port,
                        to,
                        to_port,
                    }
                };
                api::stage(&self.state, operation, offset)?;
                Ok(Value::None)
            }
            "commit" => {
                arity(0, 0)?;
                no_kwargs()?;
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| self.error(ErrKind::Runtime, "script state poisoned", offset))?;
                state.api_operations += 1;
                if state.api_operations > MAX_API_OPERATIONS {
                    return Err(self.limit_error(offset, api::API_LIMIT_MESSAGE));
                }
                state.committed = true;
                Ok(Value::None)
            }
            "has_node" => {
                arity(1, 1)?;
                no_kwargs()?;
                api::begin_query(&self.state, offset)?;
                let id = api::str_arg(&args[0], offset)?;
                Ok(Value::Bool(self.snapshot.node(&id).is_some()))
            }
            "node_info" => {
                arity(1, 1)?;
                no_kwargs()?;
                api::begin_query(&self.state, offset)?;
                let id = api::str_arg(&args[0], offset)?;
                let node = self.snapshot.node(&id).ok_or_else(|| {
                    self.error(ErrKind::Runtime, format!("unknown node `{id}`"), offset)
                })?;
                let json = serde_json::json!({
                    "id": id,
                    "kind": node.kind,
                    "spec": node.spec,
                    "ports": node.ports,
                });
                Ok(api::json_to_script_value(&json))
            }
            "node_kinds" => {
                arity(0, 0)?;
                no_kwargs()?;
                api::begin_query(&self.state, offset)?;
                let kinds = self.snapshot.node_kinds();
                let items = kinds
                    .into_iter()
                    .map(|kind| Value::Str(Rc::from(kind.as_str())))
                    .collect();
                self.make_list(items, offset)
            }
            "node_spec" => {
                arity(1, 1)?;
                no_kwargs()?;
                api::begin_query(&self.state, offset)?;
                let kind = api::str_arg(&args[0], offset)?;
                let schema = self
                    .registry
                    .get_node_spec(&kind)
                    .map_err(|error| self.error(ErrKind::Runtime, error.to_string(), offset))?;
                let json = serde_json::to_value(&schema)
                    .map_err(|error| self.error(ErrKind::Runtime, error.to_string(), offset))?;
                Ok(api::json_to_script_value(&json))
            }
            "node_ports" => {
                arity(1, 1)?;
                no_kwargs()?;
                api::begin_query(&self.state, offset)?;
                let kind = api::str_arg(&args[0], offset)?;
                let ports = self
                    .registry
                    .get_node_ports(&kind)
                    .map_err(|error| self.error(ErrKind::Runtime, error.to_string(), offset))?;
                let json = serde_json::to_value(&ports)
                    .map_err(|error| self.error(ErrKind::Runtime, error.to_string(), offset))?;
                Ok(api::json_to_script_value(&json))
            }
            "node_ports_for_spec" => {
                arity(2, 2)?;
                no_kwargs()?;
                api::begin_query(&self.state, offset)?;
                let kind = api::str_arg(&args[0], offset)?;
                let spec = api::spec_to_json(&args[1], offset)?;
                let ports = self
                    .registry
                    .get_node_ports_for_spec(&kind, spec)
                    .map_err(|error| self.error(ErrKind::Runtime, error.to_string(), offset))?;
                let json = serde_json::to_value(&ports)
                    .map_err(|error| self.error(ErrKind::Runtime, error.to_string(), offset))?;
                Ok(api::json_to_script_value(&json))
            }
            "graph_summary" => {
                arity(0, 0)?;
                no_kwargs()?;
                api::begin_query(&self.state, offset)?;
                Ok(api::json_to_script_value(&self.snapshot.summary()))
            }
            "dot" => {
                arity(0, 0)?;
                no_kwargs()?;
                api::begin_query(&self.state, offset)?;
                Ok(Value::Str(Rc::from(self.snapshot.dot())))
            }
            other => Err(self.error(
                ErrKind::Runtime,
                format!("unknown host function '{other}'"),
                offset,
            )),
        }
    }

    // ── Iteration ──────────────────────────────────────────────────────────

    fn make_iterant(&mut self, value: &Value, offset: ast::TextSize) -> EvalResult<Iterant> {
        match value {
            Value::List(list) => Ok(Iterant::Items(list.borrow().items.clone().into_iter())),
            Value::Tuple(tuple) => Ok(Iterant::Items(tuple.items.clone().into_iter())),
            Value::Str(text) => Ok(Iterant::Items(
                text.chars()
                    .map(|ch| Value::Str(Rc::from(ch.to_string().as_str())))
                    .collect::<Vec<_>>()
                    .into_iter(),
            )),
            Value::Dict(dict) => Ok(Iterant::Items(
                dict.borrow()
                    .entries
                    .iter()
                    .map(|(key, _)| key_to_value(key))
                    .collect::<Vec<_>>()
                    .into_iter(),
            )),
            Value::Range(range) => Ok(Iterant::Range {
                next: range.start,
                remaining: range_len(range) as u64,
                step: range.step,
            }),
            other => Err(self.type_error(
                offset,
                format!("'{}' object is not iterable", other.type_name()),
            )),
        }
    }

    fn collect_capped(&mut self, value: &Value, offset: ast::TextSize) -> EvalResult<Vec<Value>> {
        let mut iterant = self.make_iterant(value, offset)?;
        let mut items = Vec::new();
        while let Some(item) = iterant.next() {
            if items.len() >= MAX_ARRAY_SIZE {
                return Err(self.limit_error(offset, "array size limit exceeded"));
            }
            items.push(item);
        }
        Ok(items)
    }

    fn comp_emit(
        &mut self,
        generators: &[ast::Comprehension],
        level: usize,
        scope: &ScopeRef,
        out: &mut Vec<Value>,
        emit: &mut dyn FnMut(&mut Interp<'a>, &ScopeRef) -> EvalResult<Value>,
    ) -> EvalResult<()> {
        let Some(generator) = generators.get(level) else {
            let value = emit(self, scope)?;
            if out.len() >= MAX_ARRAY_SIZE {
                return Err(
                    self.limit_error(generator_start(generators), "array size limit exceeded")
                );
            }
            out.push(value);
            return Ok(());
        };
        if generator.is_async {
            return Err(self.unimplemented("async comprehensions", generator.iter.start()));
        }
        let iterable = self.eval_expr(&generator.iter, scope)?;
        let mut iterant = self.make_iterant(&iterable, generator.iter.start())?;
        loop {
            self.charge(generator.iter.start())?;
            let Some(item) = iterant.next() else {
                break;
            };
            self.bind_target(&generator.target, item, scope)?;
            let mut keep = true;
            for condition in &generator.ifs {
                let test = self.eval_expr(condition, scope)?;
                if !truthy(&test) {
                    keep = false;
                    break;
                }
            }
            if keep {
                self.comp_emit(generators, level + 1, scope, out, emit)?;
            }
        }
        Ok(())
    }

    fn bind_target(
        &mut self,
        target: &ast::Expr,
        value: Value,
        scope: &ScopeRef,
    ) -> EvalResult<()> {
        self.assign_to(target, value, scope)
    }

    // ── Subscripts and methods ─────────────────────────────────────────────

    fn subscript_read(
        &mut self,
        subscript: &ast::ExprSubscript,
        scope: &ScopeRef,
    ) -> EvalResult<Value> {
        let receiver = self.eval_expr(&subscript.value, scope)?;
        let offset = subscript.slice.start();
        let not_subscriptable =
            |other: &Value| format!("'{}' object is not subscriptable", other.type_name());
        if let ast::Expr::Slice(slice) = &*subscript.slice {
            let lower = self.slice_bound(slice.lower.as_deref(), scope)?;
            let upper = self.slice_bound(slice.upper.as_deref(), scope)?;
            let step = self.slice_bound(slice.step.as_deref(), scope)?;
            return match &receiver {
                Value::List(list) => {
                    let items = list.borrow().items.clone();
                    let picked = py_slice_indices(items.len(), lower, upper, step)
                        .map_err(|message| self.value_error(offset, message))?;
                    let out: Vec<Value> = picked
                        .into_iter()
                        .map(|index| items[index].clone())
                        .collect();
                    self.make_list(out, subscript.start())
                }
                Value::Tuple(tuple) => {
                    let picked = py_slice_indices(tuple.items.len(), lower, upper, step)
                        .map_err(|message| self.value_error(offset, message))?;
                    let out: Vec<Value> = picked
                        .into_iter()
                        .map(|index| tuple.items[index].clone())
                        .collect();
                    self.make_tuple(out, subscript.start())
                }
                Value::Str(text) => {
                    let chars: Vec<char> = text.chars().collect();
                    let picked = py_slice_indices(chars.len(), lower, upper, step)
                        .map_err(|message| self.value_error(offset, message))?;
                    let out: String = picked.iter().map(|&index| chars[index]).collect();
                    self.make_string(out, subscript.start())
                }
                other => Err(self.type_error(subscript.start(), not_subscriptable(other))),
            };
        }
        let index_value = self.eval_expr(&subscript.slice, scope)?;
        match &receiver {
            Value::Dict(dict) => {
                let key = self.dict_key(&index_value, offset)?;
                let dict = dict.borrow();
                dict.entries
                    .iter()
                    .find(|(candidate, _)| candidate == &key)
                    .map(|(_, value)| value.clone())
                    .ok_or_else(|| {
                        self.error(ErrKind::Key, format!("key {}", key.to_display()), offset)
                    })
            }
            Value::List(list) => {
                let index = self.int_index(&index_value, offset)?;
                let list = list.borrow();
                let resolved = resolve_index(index, list.items.len())
                    .map_err(|error| self.error(ErrKind::Index, error, offset))?;
                Ok(list.items[resolved].clone())
            }
            Value::Tuple(tuple) => {
                let index = self.int_index(&index_value, offset)?;
                let resolved = resolve_index(index, tuple.items.len())
                    .map_err(|error| self.error(ErrKind::Index, error, offset))?;
                Ok(tuple.items[resolved].clone())
            }
            Value::Str(text) => {
                let index = self.int_index(&index_value, offset)?;
                let chars: Vec<char> = text.chars().collect();
                let resolved = resolve_index(index, chars.len())
                    .map_err(|error| self.error(ErrKind::Index, error, offset))?;
                Ok(Value::Str(Rc::from(chars[resolved].to_string().as_str())))
            }
            other => Err(self.type_error(subscript.start(), not_subscriptable(other))),
        }
    }

    fn slice_bound(
        &mut self,
        bound: Option<&ast::Expr>,
        scope: &ScopeRef,
    ) -> EvalResult<Option<i64>> {
        let Some(bound) = bound else {
            return Ok(None);
        };
        let value = self.eval_expr(bound, scope)?;
        match value {
            Value::None => Ok(None),
            Value::Int(index) => Ok(Some(index)),
            Value::Bool(value) => Ok(Some(i64::from(value))),
            other => Err(self.type_error(
                bound.start(),
                format!(
                    "slice indices must be integers, not '{}'",
                    other.type_name()
                ),
            )),
        }
    }

    fn call_method(
        &mut self,
        receiver: Value,
        attr: &str,
        args: &[Value],
        kwargs: &[(String, Value)],
        offset: ast::TextSize,
    ) -> EvalResult<Value> {
        if !kwargs.is_empty() {
            return Err(self.type_error(offset, "methods take no keyword arguments"));
        }
        let arity = |expected: usize| -> EvalResult<()> {
            if args.len() != expected {
                return Err(self.type_error(
                    offset,
                    format!(
                        "{}.{}() takes {} argument{}, got {}",
                        receiver.type_name(),
                        attr,
                        expected,
                        if expected == 1 { "" } else { "s" },
                        args.len()
                    ),
                ));
            }
            Ok(())
        };
        match (&receiver, attr) {
            (Value::Str(text), "join") => {
                arity(1)?;
                let items = self.collect_capped(&args[0], offset)?;
                let mut out = String::new();
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(text);
                    }
                    match item {
                        Value::Str(part) => out.push_str(part),
                        other => {
                            return Err(self.type_error(
                                offset,
                                format!("join expects str items, got '{}'", other.type_name()),
                            ));
                        }
                    }
                }
                self.make_string(out, offset)
            }
            (Value::Str(text), "split") => {
                if args.len() > 1 {
                    return Err(self.type_error(offset, "split() supports at most a separator"));
                }
                let parts: Vec<String> = match args.first() {
                    Some(Value::Str(sep)) if !sep.is_empty() => {
                        text.split(sep.as_ref()).map(str::to_string).collect()
                    }
                    Some(Value::Str(_)) => return Err(self.value_error(offset, "empty separator")),
                    Some(other) => {
                        return Err(self.type_error(
                            offset,
                            format!("separator must be str, not '{}'", other.type_name()),
                        ));
                    }
                    None => text.split_whitespace().map(str::to_string).collect(),
                };
                let items = parts
                    .into_iter()
                    .map(|part| Value::Str(Rc::from(part.as_str())))
                    .collect();
                self.make_list(items, offset)
            }
            (Value::Str(text), "strip") => {
                arity(0)?;
                self.make_string(text.trim().to_string(), offset)
            }
            (Value::Str(text), "startswith") | (Value::Str(text), "endswith") => {
                arity(1)?;
                let Value::Str(needle) = &args[0] else {
                    return Err(self.type_error(offset, "expected a str argument"));
                };
                let matched = if attr == "startswith" {
                    text.starts_with(needle.as_ref())
                } else {
                    text.ends_with(needle.as_ref())
                };
                Ok(Value::Bool(matched))
            }
            (Value::Str(text), "replace") => {
                arity(2)?;
                let (Value::Str(from), Value::Str(to)) = (&args[0], &args[1]) else {
                    return Err(self.type_error(offset, "replace expects str arguments"));
                };
                self.make_string(text.replace(from.as_ref(), to.as_ref()), offset)
            }
            (Value::Str(text), "lower") => {
                arity(0)?;
                self.make_string(text.to_lowercase(), offset)
            }
            (Value::Str(text), "upper") => {
                arity(0)?;
                self.make_string(text.to_uppercase(), offset)
            }
            (Value::Str(text), "count") => {
                arity(1)?;
                let Value::Str(needle) = &args[0] else {
                    return Err(self.type_error(offset, "expected a str argument"));
                };
                Ok(Value::Int(text.matches(needle.as_ref()).count() as i64))
            }
            (Value::List(list), "append") => {
                arity(1)?;
                self.check_value_depth(&args[0], offset)?;
                let mut list = list.borrow_mut();
                if list.items.len() >= MAX_ARRAY_SIZE {
                    return Err(self.limit_error(offset, "array size limit exceeded"));
                }
                list.depth = list.depth.max(args[0].depth() + 1);
                list.items.push(args[0].clone());
                Ok(Value::None)
            }
            (Value::List(list), "extend") => {
                arity(1)?;
                let items = self.collect_capped(&args[0], offset)?;
                let mut list = list.borrow_mut();
                if list.items.len() + items.len() > MAX_ARRAY_SIZE {
                    return Err(self.limit_error(offset, "array size limit exceeded"));
                }
                let addition = items.iter().map(Value::depth).max().unwrap_or(0) + 1;
                let candidate = list.depth.max(addition);
                if candidate > MAX_VALUE_DEPTH {
                    return Err(self.limit_error(offset, "value nesting too deep"));
                }
                list.depth = candidate;
                list.items.extend(items);
                Ok(Value::None)
            }
            (Value::List(list), "insert") => {
                if args.len() != 2 {
                    return Err(self.type_error(offset, "insert() takes (index, value)"));
                }
                let index = self.int_index(&args[0], offset)?;
                self.check_value_depth(&args[1], offset)?;
                let mut list = list.borrow_mut();
                if list.items.len() >= MAX_ARRAY_SIZE {
                    return Err(self.limit_error(offset, "array size limit exceeded"));
                }
                list.depth = list.depth.max(args[1].depth() + 1);
                let resolved = index.clamp(0, list.items.len() as i64) as usize;
                list.items.insert(resolved, args[1].clone());
                Ok(Value::None)
            }
            (Value::List(list), "pop") => {
                if args.len() > 1 {
                    return Err(self.type_error(offset, "pop() takes at most one index"));
                }
                let mut list = list.borrow_mut();
                let resolved = if args.is_empty() {
                    if list.items.is_empty() {
                        return Err(self.error(ErrKind::Index, "pop from empty list", offset));
                    }
                    list.items.len() - 1
                } else {
                    let index = self.int_index(&args[0], offset)?;
                    resolve_index(index, list.items.len())
                        .map_err(|error| self.error(ErrKind::Index, error, offset))?
                };
                Ok(list.items.remove(resolved))
            }
            (Value::List(list), "count") => {
                arity(1)?;
                let list = list.borrow();
                Ok(Value::Int(
                    list.items
                        .iter()
                        .filter(|item| value_eq(item, &args[0]))
                        .count() as i64,
                ))
            }
            (Value::Dict(dict), "get") => {
                if args.is_empty() || args.len() > 2 {
                    return Err(self.type_error(offset, "get() takes a key and optional default"));
                }
                let key = self.dict_key(&args[0], offset)?;
                let dict = dict.borrow();
                match dict.entries.iter().find(|(candidate, _)| candidate == &key) {
                    Some((_, value)) => Ok(value.clone()),
                    None => Ok(args.get(1).cloned().unwrap_or(Value::None)),
                }
            }
            (Value::Dict(dict), "keys") => {
                arity(0)?;
                let dict = dict.borrow();
                let items = dict
                    .entries
                    .iter()
                    .map(|(key, _)| key_to_value(key))
                    .collect();
                self.make_list(items, offset)
            }
            (Value::Dict(dict), "values") => {
                arity(0)?;
                let dict = dict.borrow();
                let items = dict
                    .entries
                    .iter()
                    .map(|(_, value)| value.clone())
                    .collect();
                self.make_list(items, offset)
            }
            (Value::Dict(dict), "items") => {
                arity(0)?;
                let dict = dict.borrow();
                let mut items = Vec::with_capacity(dict.entries.len());
                for (key, value) in &dict.entries {
                    items.push(Value::Tuple(Rc::new(TupleData {
                        items: vec![key_to_value(key), value.clone()],
                        depth: value.depth().saturating_add(1).max(2),
                    })));
                }
                self.make_list(items, offset)
            }
            (Value::Dict(dict), "pop") => {
                if args.is_empty() || args.len() > 2 {
                    return Err(self.type_error(offset, "pop() takes a key and optional default"));
                }
                let key = self.dict_key(&args[0], offset)?;
                let mut dict = dict.borrow_mut();
                match dict_remove(&mut dict, &key) {
                    Some(value) => Ok(value),
                    None => match args.get(1) {
                        Some(default) => Ok(default.clone()),
                        None => Err(self.error(
                            ErrKind::Key,
                            format!("key {}", key.to_display()),
                            offset,
                        )),
                    },
                }
            }
            (receiver, attr) => Err(self.attribute_error(receiver, attr, offset)),
        }
    }

    // ── Containers, primitives ─────────────────────────────────────────────

    fn check_value_depth(&self, value: &Value, offset: ast::TextSize) -> EvalResult<()> {
        if value.depth() + 1 > MAX_VALUE_DEPTH {
            return Err(self.limit_error(offset, "value nesting too deep"));
        }
        Ok(())
    }

    fn make_list(&self, items: Vec<Value>, offset: ast::TextSize) -> EvalResult<Value> {
        if items.len() > MAX_ARRAY_SIZE {
            return Err(self.limit_error(offset, "array size limit exceeded"));
        }
        let depth = items.iter().map(Value::depth).max().unwrap_or(0) + 1;
        if depth > MAX_VALUE_DEPTH {
            return Err(self.limit_error(offset, "value nesting too deep"));
        }
        Ok(Value::List(Rc::new(RefCell::new(ListData {
            items,
            depth,
        }))))
    }

    fn make_tuple(&self, items: Vec<Value>, offset: ast::TextSize) -> EvalResult<Value> {
        if items.len() > MAX_ARRAY_SIZE {
            return Err(self.limit_error(offset, "array size limit exceeded"));
        }
        let depth = items.iter().map(Value::depth).max().unwrap_or(0) + 1;
        if depth > MAX_VALUE_DEPTH {
            return Err(self.limit_error(offset, "value nesting too deep"));
        }
        Ok(Value::Tuple(Rc::new(TupleData { items, depth })))
    }

    fn make_dict(
        &self,
        entries: Vec<(DictKey, Value)>,
        offset: ast::TextSize,
    ) -> EvalResult<Value> {
        if entries.len() > MAX_MAP_SIZE {
            return Err(self.limit_error(offset, "map size limit exceeded"));
        }
        let mut dict = DictData {
            entries: Vec::new(),
            index: HashMap::new(),
            depth: 1,
        };
        for (key, value) in entries {
            let candidate = value.depth() + 1;
            if candidate > MAX_VALUE_DEPTH {
                return Err(self.limit_error(offset, "value nesting too deep"));
            }
            dict.depth = dict.depth.max(candidate);
            dict_insert(&mut dict, key, value);
        }
        Ok(Value::Dict(Rc::new(RefCell::new(dict))))
    }

    fn make_string(&self, text: String, offset: ast::TextSize) -> EvalResult<Value> {
        if text.len() > MAX_STRING_SIZE {
            return Err(self.limit_error(offset, "string size limit exceeded"));
        }
        Ok(Value::Str(Rc::from(text.as_str())))
    }

    fn dict_key(&self, value: &Value, offset: ast::TextSize) -> EvalResult<DictKey> {
        DictKey::from_value(value).map_err(|message| self.type_error(offset, message))
    }

    fn int_index(&self, value: &Value, offset: ast::TextSize) -> EvalResult<i64> {
        match value {
            Value::Int(index) => Ok(*index),
            Value::Bool(b) => Ok(i64::from(*b)),
            other => Err(self.type_error(
                offset,
                format!("indices must be integers, not '{}'", other.type_name()),
            )),
        }
    }

    fn to_display(&self, value: &Value) -> String {
        match value {
            Value::None => "None".into(),
            Value::Bool(b) => {
                if *b {
                    "True".into()
                } else {
                    "False".into()
                }
            }
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format_float(*f),
            Value::Str(s) => s.to_string(),
            Value::List(_) | Value::Tuple(_) | Value::Dict(_) | Value::Range(_) => self.repr(value),
            Value::Function(function) => format!("<function {}>", function.name),
            Value::Builtin(name) => format!("<built-in function {name}>"),
            Value::Pair(_, _) => "<pair>".into(),
        }
    }

    fn attribute_error(&self, receiver: &Value, attr: &str, offset: ast::TextSize) -> EvalError {
        self.error(
            ErrKind::Attribute,
            format!(
                "'{}' object has no attribute '{}'",
                receiver.type_name(),
                attr
            ),
            offset,
        )
    }

    fn repr(&self, value: &Value) -> String {
        match value {
            Value::Str(text) => format!("'{}'", escape_repr(text)),
            Value::List(list) => {
                let items: Vec<String> = list
                    .borrow()
                    .items
                    .iter()
                    .map(|item| self.repr(item))
                    .collect();
                format!("[{}]", items.join(", "))
            }
            Value::Tuple(tuple) => {
                let items: Vec<String> = tuple.items.iter().map(|item| self.repr(item)).collect();
                if items.len() == 1 {
                    format!("({},)", items[0])
                } else {
                    format!("({})", items.join(", "))
                }
            }
            Value::Dict(dict) => {
                let entries: Vec<String> = dict
                    .borrow()
                    .entries
                    .iter()
                    .map(|(key, value)| {
                        let key = match key {
                            DictKey::Str(text) => format!("'{}'", escape_repr(text)),
                            other => other.to_display(),
                        };
                        format!("{}: {}", key, self.repr(value))
                    })
                    .collect();
                format!("{{{}}}", entries.join(", "))
            }
            Value::Range(range) => {
                if range.step == 1 {
                    format!("range({}, {})", range.start, range.stop)
                } else {
                    format!("range({}, {}, {})", range.start, range.stop, range.step)
                }
            }
            other => self.to_display(other),
        }
    }

    // ── Operators ──────────────────────────────────────────────────────────

    fn bin_op(
        &mut self,
        op: ast::Operator,
        left: Value,
        right: Value,
        offset: ast::TextSize,
    ) -> EvalResult<Value> {
        use ast::Operator::*;
        let arity_error = |a: &Value, b: &Value, symbol: &str| {
            self.type_error(
                offset,
                format!(
                    "unsupported operand type(s) for {symbol}: '{}' and '{}'",
                    a.type_name(),
                    b.type_name()
                ),
            )
        };
        match op {
            Add => match (left, right) {
                (Value::Int(a), Value::Int(b)) => a
                    .checked_add(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.limit_error(offset, "integer overflow")),
                (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 + b)),
                (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + b as f64)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                (Value::Str(a), Value::Str(b)) => {
                    let mut out = String::with_capacity(a.len() + b.len());
                    out.push_str(&a);
                    out.push_str(&b);
                    self.make_string(out, offset)
                }
                (Value::List(a), Value::List(b)) => {
                    let mut items = a.borrow().items.clone();
                    items.extend(b.borrow().items.iter().cloned());
                    self.make_list(items, offset)
                }
                (Value::Tuple(a), Value::Tuple(b)) => {
                    let mut items = a.items.clone();
                    items.extend(b.items.iter().cloned());
                    self.make_tuple(items, offset)
                }
                (a, b) => Err(arity_error(&a, &b, "+")),
            },
            Sub => match (left, right) {
                (Value::Int(a), Value::Int(b)) => a
                    .checked_sub(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.limit_error(offset, "integer overflow")),
                (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 - b)),
                (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a - b as f64)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                (a, b) => Err(arity_error(&a, &b, "-")),
            },
            Mult => match (left, right) {
                (Value::Int(a), Value::Int(b)) => a
                    .checked_mul(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.limit_error(offset, "integer overflow")),
                (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 * b)),
                (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * b as f64)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
                (Value::Str(text), Value::Int(count)) | (Value::Int(count), Value::Str(text)) => {
                    let count = usize::try_from(count).unwrap_or(usize::MAX);
                    if text.len().saturating_mul(count) > MAX_STRING_SIZE {
                        return Err(self.limit_error(offset, "string size limit exceeded"));
                    }
                    self.make_string(text.repeat(count), offset)
                }
                (Value::List(list), Value::Int(count)) | (Value::Int(count), Value::List(list)) => {
                    let count = usize::try_from(count).unwrap_or(usize::MAX);
                    let items = list.borrow().items.clone();
                    if items.len().saturating_mul(count) > MAX_ARRAY_SIZE {
                        return Err(self.limit_error(offset, "array size limit exceeded"));
                    }
                    let mut out = Vec::with_capacity(items.len().saturating_mul(count));
                    for _ in 0..count {
                        out.extend(items.iter().cloned());
                    }
                    self.make_list(out, offset)
                }
                (a, b) => Err(arity_error(&a, &b, "*")),
            },
            Div => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => {
                    if *b == 0 {
                        return Err(self.error(ErrKind::ZeroDivision, "division by zero", offset));
                    }
                    Ok(Value::Float(*a as f64 / *b as f64))
                }
                (a, b) => match (numeric(a), numeric(b)) {
                    (Some(x), Some(y)) => {
                        if y == 0.0 {
                            return Err(self.error(
                                ErrKind::ZeroDivision,
                                "division by zero",
                                offset,
                            ));
                        }
                        Ok(Value::Float(x / y))
                    }
                    _ => Err(arity_error(a, b, "/")),
                },
            },
            FloorDiv | Mod => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => {
                    if *b == 0 {
                        return Err(self.error(ErrKind::ZeroDivision, "division by zero", offset));
                    }
                    if *a == i64::MIN && *b == -1 {
                        return Err(self.limit_error(offset, "integer overflow"));
                    }
                    let (quotient, remainder) = py_div_rem(*a, *b);
                    Ok(if op == FloorDiv {
                        Value::Int(quotient)
                    } else {
                        Value::Int(remainder)
                    })
                }
                (a, b) => match (numeric(a), numeric(b)) {
                    (Some(x), Some(y)) => {
                        if y == 0.0 {
                            return Err(self.error(
                                ErrKind::ZeroDivision,
                                "division by zero",
                                offset,
                            ));
                        }
                        let divided = (x / y).floor();
                        Ok(if op == FloorDiv {
                            Value::Float(divided)
                        } else {
                            Value::Float(x - divided * y)
                        })
                    }
                    _ => Err(arity_error(a, b, if op == FloorDiv { "//" } else { "%" })),
                },
            },
            Pow => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => {
                    if *b < 0 {
                        return Ok(Value::Float(
                            (*a as f64).powi((*b).try_into().unwrap_or(i32::MAX)),
                        ));
                    }
                    let exponent = u32::try_from(*b).unwrap_or(u32::MAX);
                    if exponent > 64 {
                        return Err(self.limit_error(offset, "exponent too large"));
                    }
                    a.checked_pow(exponent)
                        .map(Value::Int)
                        .ok_or_else(|| self.limit_error(offset, "integer overflow"))
                }
                (a, b) => match (numeric(a), numeric(b)) {
                    (Some(x), Some(y)) => Ok(Value::Float(x.powf(y))),
                    _ => Err(arity_error(a, b, "**")),
                },
            },
            LShift => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => {
                    if *b < 0 || *b >= 64 {
                        return Err(self.limit_error(offset, "shift too large"));
                    }
                    a.checked_shl(*b as u32)
                        .map(Value::Int)
                        .ok_or_else(|| self.limit_error(offset, "integer overflow"))
                }
                (a, b) => Err(arity_error(a, b, "<<")),
            },
            RShift => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => {
                    if *b < 0 {
                        return Err(self.limit_error(offset, "shift too large"));
                    }
                    if *b >= 64 {
                        Ok(Value::Int(if *a < 0 { -1 } else { 0 }))
                    } else {
                        Ok(Value::Int(a >> *b))
                    }
                }
                (a, b) => Err(arity_error(a, b, ">>")),
            },
            BitOr | BitXor | BitAnd => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Int(match op {
                    BitOr => a | b,
                    BitXor => a ^ b,
                    _ => a & b,
                })),
                (a, b) => Err(arity_error(a, b, "&")),
            },
            MatMult => Err(self.unimplemented("the '@' operator", offset)),
        }
    }

    fn compare(
        &mut self,
        op: &ast::CmpOp,
        left: &Value,
        right: &Value,
        offset: ast::TextSize,
    ) -> EvalResult<bool> {
        use ast::CmpOp::*;
        match op {
            Eq => Ok(value_eq(left, right)),
            NotEq => Ok(!value_eq(left, right)),
            Lt | LtE | Gt | GtE => {
                let ordering = self.value_order(left, right, offset)?;
                Ok(match op {
                    Lt => ordering == Ordering::Less,
                    LtE => ordering != Ordering::Greater,
                    Gt => ordering == Ordering::Greater,
                    _ => ordering != Ordering::Less,
                })
            }
            Is | IsNot => {
                let ((Value::None, _) | (_, Value::None)) = (left, right) else {
                    return Err(self.type_error(
                        offset,
                        "identity comparison ('is') is only supported against None",
                    ));
                };
                let same = matches!(left, Value::None) && matches!(right, Value::None);
                Ok(if *op == Is { same } else { !same })
            }
            In | NotIn => {
                let contains = match right {
                    Value::List(list) => {
                        list.borrow().items.iter().any(|item| value_eq(item, left))
                    }
                    Value::Tuple(tuple) => tuple.items.iter().any(|item| value_eq(item, left)),
                    Value::Dict(dict) => {
                        let key = self.dict_key(left, offset)?;
                        dict.borrow()
                            .entries
                            .iter()
                            .any(|(candidate, _)| candidate == &key)
                    }
                    Value::Str(haystack) => match left {
                        Value::Str(needle) => haystack.contains(needle.as_ref()),
                        other => {
                            return Err(self.type_error(
                                offset,
                                format!("'in <str>' requires str, not '{}'", other.type_name()),
                            ));
                        }
                    },
                    Value::Range(range) => range_contains(range, left)?,
                    other => {
                        return Err(self.type_error(
                            offset,
                            format!(
                                "argument of type '{}' is not a container",
                                other.type_name()
                            ),
                        ));
                    }
                };
                Ok(if *op == In { contains } else { !contains })
            }
        }
    }

    fn value_order(
        &mut self,
        left: &Value,
        right: &Value,
        offset: ast::TextSize,
    ) -> EvalResult<Ordering> {
        if let (Value::Str(a), Value::Str(b)) = (left, right) {
            return Ok(a.cmp(b));
        }
        if let (Some(a), Some(b)) = (numeric(left), numeric(right)) {
            return Ok(a.partial_cmp(&b).unwrap_or(Ordering::Equal));
        }
        if let (Some(a), Some(b)) = (as_items(left), as_items(right)) {
            return self.sequence_order(a, b, offset);
        }
        Err(self.type_error(
            offset,
            format!(
                "'<' not supported between '{}' and '{}'",
                left.type_name(),
                right.type_name()
            ),
        ))
    }

    fn sequence_order(
        &mut self,
        left: Vec<Value>,
        right: Vec<Value>,
        offset: ast::TextSize,
    ) -> EvalResult<Ordering> {
        for (a, b) in left.iter().zip(right.iter()) {
            let ordering = self.value_order(a, b, offset)?;
            if ordering != Ordering::Equal {
                return Ok(ordering);
            }
        }
        Ok(left.len().cmp(&right.len()))
    }

    // ── Pure builtins ──────────────────────────────────────────────────────

    fn call_builtin(
        &mut self,
        name: &'static str,
        args: &[Value],
        kwargs: &[(String, Value)],
        offset: ast::TextSize,
    ) -> EvalResult<Value> {
        if !kwargs.is_empty() && name != "enumerate" {
            return Err(self.type_error(offset, format!("{name}() takes no keyword arguments")));
        }
        let first = || -> EvalResult<&Value> {
            args.first()
                .ok_or_else(|| self.type_error(offset, format!("{name}() expects an argument")))
        };
        match name {
            "len" => match args.first().and_then(Value::len) {
                Some(len) => Ok(Value::Int(len as i64)),
                None => {
                    let type_name = args.first().map(Value::type_name).unwrap_or("missing");
                    Err(self.type_error(
                        offset,
                        format!("len() expects a sized object, got '{type_name}'"),
                    ))
                }
            },
            "str" => {
                if args.len() > 1 {
                    return Err(self.type_error(offset, "str() takes at most one argument"));
                }
                let text = match args.first() {
                    Some(value) => self.to_display(value),
                    None => String::new(),
                };
                self.make_string(text, offset)
            }
            "int" => match args.first() {
                None => Ok(Value::Int(0)),
                Some(Value::Int(value)) => Ok(Value::Int(*value)),
                Some(Value::Bool(value)) => Ok(Value::Int(i64::from(*value))),
                Some(Value::Float(value)) => {
                    if value.is_nan() || value.is_infinite() {
                        return Err(self.value_error(
                            offset,
                            "cannot convert float NaN or infinity to integer",
                        ));
                    }
                    Ok(Value::Int(value.trunc() as i64))
                }
                Some(Value::Str(text)) => {
                    text.trim().parse::<i64>().map(Value::Int).map_err(|_| {
                        self.value_error(offset, format!("invalid literal for int(): '{text}'"))
                    })
                }
                Some(other) => Err(self.type_error(
                    offset,
                    format!(
                        "int() expects a number or string, got '{}'",
                        other.type_name()
                    ),
                )),
            },
            "float" => match args.first() {
                None => Ok(Value::Float(0.0)),
                Some(Value::Int(value)) => Ok(Value::Float(*value as f64)),
                Some(Value::Bool(value)) => Ok(Value::Float(f64::from(u8::from(*value)))),
                Some(Value::Float(value)) => Ok(Value::Float(*value)),
                Some(Value::Str(text)) => {
                    let lowered = text.trim().to_lowercase();
                    let parsed = match lowered.as_str() {
                        "inf" | "+inf" | "infinity" | "+infinity" => f64::INFINITY,
                        "-inf" | "-infinity" => f64::NEG_INFINITY,
                        "nan" | "+nan" | "-nan" => f64::NAN,
                        other => other.parse::<f64>().map_err(|_| {
                            self.value_error(
                                offset,
                                format!("invalid literal for float(): '{text}'"),
                            )
                        })?,
                    };
                    Ok(Value::Float(parsed))
                }
                Some(other) => Err(self.type_error(
                    offset,
                    format!(
                        "float() expects a number or string, got '{}'",
                        other.type_name()
                    ),
                )),
            },
            "bool" => {
                let result = match args.first() {
                    Some(value) => truthy(value),
                    None => false,
                };
                Ok(Value::Bool(result))
            }
            "abs" => match args.first() {
                Some(Value::Int(value)) => value
                    .checked_abs()
                    .map(Value::Int)
                    .ok_or_else(|| self.limit_error(offset, "integer overflow")),
                Some(Value::Float(value)) => Ok(Value::Float(value.abs())),
                Some(other) => Err(self.type_error(
                    offset,
                    format!("abs() expects a number, got '{}'", other.type_name()),
                )),
                None => Err(self.type_error(offset, "abs() takes one argument")),
            },
            "min" | "max" => {
                let items = if args.len() == 1 {
                    self.collect_capped(&args[0], offset)?
                } else {
                    args.to_vec()
                };
                let mut best: Option<Value> = None;
                for item in items {
                    best = Some(match best {
                        None => item,
                        Some(current) => {
                            let ordering = self.value_order(&item, &current, offset)?;
                            let take = if name == "min" {
                                ordering == Ordering::Less
                            } else {
                                ordering == Ordering::Greater
                            };
                            if take { item } else { current }
                        }
                    });
                }
                best.ok_or_else(|| {
                    self.value_error(offset, format!("{name}() arg is an empty sequence"))
                })
            }
            "sum" => {
                let start = args.get(1).cloned().unwrap_or(Value::Int(0));
                let items = self.collect_capped(first()?, offset)?;
                let mut total = start;
                for item in items {
                    total = self.bin_op_plain_add(&total, &item, offset)?;
                }
                Ok(total)
            }
            "round" => match args.first() {
                Some(Value::Int(value)) => Ok(Value::Int(*value)),
                Some(Value::Float(value)) => match args.get(1) {
                    None | Some(Value::None) => Ok(Value::Int(value.round_ties_even() as i64)),
                    Some(Value::Int(digits)) => {
                        let factor = 10f64.powi(*digits as i32);
                        Ok(Value::Float((value * factor).round_ties_even() / factor))
                    }
                    Some(other) => Err(self.type_error(
                        offset,
                        format!("round() ndigits must be int, not '{}'", other.type_name()),
                    )),
                },
                Some(other) => Err(self.type_error(
                    offset,
                    format!("round() expects a number, got '{}'", other.type_name()),
                )),
                None => Err(self.type_error(offset, "round() takes one or two arguments")),
            },
            "sorted" => {
                let mut items = self.collect_capped(first()?, offset)?;
                // Stable insertion sort; the deadline is re-checked each outer
                // pass so adversarial inputs cannot run past the budget.
                for i in 1..items.len() {
                    self.charge(offset)?;
                    let mut j = i;
                    while j > 0
                        && self.value_order(&items[j], &items[j - 1], offset)? == Ordering::Less
                    {
                        items.swap(j, j - 1);
                        j -= 1;
                    }
                }
                self.make_list(items, offset)
            }
            "reversed" => {
                let mut items = self.collect_capped(first()?, offset)?;
                items.reverse();
                self.make_list(items, offset)
            }
            "list" => {
                let items = match args.first() {
                    Some(value) => self.collect_capped(value, offset)?,
                    None => Vec::new(),
                };
                self.make_list(items, offset)
            }
            "range" => {
                let (start, stop, step) = match args.len() {
                    1 => (0, self.int_index(&args[0], offset)?, 1),
                    2 => (
                        self.int_index(&args[0], offset)?,
                        self.int_index(&args[1], offset)?,
                        1,
                    ),
                    3 => (
                        self.int_index(&args[0], offset)?,
                        self.int_index(&args[1], offset)?,
                        self.int_index(&args[2], offset)?,
                    ),
                    _ => return Err(self.type_error(offset, "range() takes 1 to 3 arguments")),
                };
                if step == 0 {
                    return Err(self.value_error(offset, "range() step must not be zero"));
                }
                Ok(Value::Range(Rc::new(RangeData { start, stop, step })))
            }
            "enumerate" => {
                let start = match kwargs.iter().find(|(name, _)| name == "start") {
                    Some((_, value)) => self.int_index(value, offset)?,
                    None => 0,
                };
                let items = self.collect_capped(first()?, offset)?;
                let mut out = Vec::with_capacity(items.len());
                for (index, item) in items.into_iter().enumerate() {
                    out.push(Value::Tuple(Rc::new(TupleData {
                        items: vec![Value::Int(start + index as i64), item.clone()],
                        depth: item.depth().saturating_add(1).max(2),
                    })));
                }
                self.make_list(out, offset)
            }
            "zip" => {
                let mut columns = Vec::with_capacity(args.len());
                for arg in args {
                    columns.push(self.collect_capped(arg, offset)?);
                }
                let shortest = columns.iter().map(Vec::len).min().unwrap_or(0);
                let mut out = Vec::with_capacity(shortest);
                for index in 0..shortest {
                    let items: Vec<Value> =
                        columns.iter().map(|column| column[index].clone()).collect();
                    let depth = items
                        .iter()
                        .map(Value::depth)
                        .max()
                        .unwrap_or(0)
                        .saturating_add(1)
                        .max(2);
                    out.push(Value::Tuple(Rc::new(TupleData { items, depth })));
                }
                self.make_list(out, offset)
            }
            other => Err(self.error(
                ErrKind::Name,
                format!("name '{other}' is not defined"),
                offset,
            )),
        }
    }

    fn bin_op_plain_add(
        &mut self,
        left: &Value,
        right: &Value,
        offset: ast::TextSize,
    ) -> EvalResult<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => a
                .checked_add(*b)
                .map(Value::Int)
                .ok_or_else(|| self.limit_error(offset, "integer overflow")),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + *b as f64)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (a, b) => Err(self.type_error(
                offset,
                format!(
                    "unsupported operand type(s) for +: '{}' and '{}'",
                    a.type_name(),
                    b.type_name()
                ),
            )),
        }
    }
}

// ── Free helpers ───────────────────────────────────────────────────────────

fn numeric(value: &Value) -> Option<f64> {
    match value {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn as_items(value: &Value) -> Option<Vec<Value>> {
    match value {
        Value::List(list) => Some(list.borrow().items.clone()),
        Value::Tuple(tuple) => Some(tuple.items.clone()),
        _ => None,
    }
}

fn value_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::None, Value::None) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::List(x), Value::List(y)) => {
            Rc::ptr_eq(x, y)
                || (x.borrow().items.len() == y.borrow().items.len()
                    && x.borrow()
                        .items
                        .iter()
                        .zip(y.borrow().items.iter())
                        .all(|(i, j)| value_eq(i, j)))
        }
        (Value::Tuple(x), Value::Tuple(y)) => {
            Rc::ptr_eq(x, y)
                || (x.items.len() == y.items.len()
                    && x.items
                        .iter()
                        .zip(y.items.iter())
                        .all(|(i, j)| value_eq(i, j)))
        }
        (Value::Dict(x), Value::Dict(y)) => {
            let x = x.borrow();
            let y = y.borrow();
            x.entries.len() == y.entries.len()
                && x.entries.iter().all(|(key, value)| {
                    y.entries
                        .iter()
                        .find(|(other_key, _)| other_key == key)
                        .is_some_and(|(_, other_value)| value_eq(value, other_value))
                })
        }
        (Value::Range(x), Value::Range(y)) => {
            range_len(x) == range_len(y)
                && (range_len(x) == 0 || (x.start == y.start && x.step == y.step))
        }
        _ => match (numeric(a), numeric(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

fn resolve_index(index: i64, len: usize) -> Result<usize, String> {
    if index < 0 {
        let resolved = (len as i64) + index;
        if resolved < 0 {
            return Err(format!("index out of range: {index}"));
        }
        Ok(resolved as usize)
    } else if (index as usize) < len {
        Ok(index as usize)
    } else {
        Err(format!("index out of range: {index}"))
    }
}

fn py_div_rem(a: i64, b: i64) -> (i64, i64) {
    let mut quotient = a / b;
    let mut remainder = a % b;
    if remainder != 0 && (remainder < 0) != (b < 0) {
        quotient -= 1;
        remainder += b;
    }
    (quotient, remainder)
}

fn py_slice_indices(
    len: usize,
    start: Option<i64>,
    stop: Option<i64>,
    step: Option<i64>,
) -> Result<Vec<usize>, String> {
    let step = step.unwrap_or(1);
    if step == 0 {
        return Err("slice step cannot be zero".into());
    }
    let len_i = len as i64;
    let clamp = |value: i64| -> i64 { value.clamp(-len_i, len_i) };
    let mut indices = Vec::new();
    if step > 0 {
        let start = start.map(clamp).unwrap_or(0).max(0);
        let stop = stop.map(clamp).unwrap_or(len_i).max(0);
        let mut cursor = start;
        while cursor < stop {
            indices.push(cursor as usize);
            cursor += step;
        }
    } else {
        let start = start.map(clamp).unwrap_or(len_i - 1).min(len_i - 1);
        let stop = stop.map(clamp).unwrap_or(-(len_i + 1));
        let mut cursor = start;
        while stop < cursor {
            if cursor >= 0 && (cursor as usize) < len {
                indices.push(cursor as usize);
            }
            cursor += step;
        }
    }
    Ok(indices)
}

fn range_contains(range: &RangeData, value: &Value) -> EvalResult<bool> {
    let Value::Int(candidate) = value else {
        return Ok(false);
    };
    let contained = if range.step > 0 {
        *candidate >= range.start
            && *candidate < range.stop
            && (*candidate - range.start) % range.step == 0
    } else {
        *candidate <= range.start
            && *candidate > range.stop
            && (range.start - *candidate) % (-range.step) == 0
    };
    Ok(contained)
}

fn key_to_value(key: &DictKey) -> Value {
    match key {
        DictKey::None => Value::None,
        DictKey::Bool(b) => Value::Bool(*b),
        DictKey::Int(i) => Value::Int(*i),
        DictKey::Str(s) => Value::Str(Rc::clone(s)),
        DictKey::Tuple(keys) => Value::Tuple(Rc::new(TupleData {
            items: keys.iter().map(key_to_value).collect(),
            depth: 1,
        })),
    }
}

fn dict_insert(dict: &mut DictData, key: DictKey, value: Value) {
    if let Some(&position) = dict.index.get(&key) {
        dict.entries[position].1 = value;
    } else {
        dict.index.insert(key.clone(), dict.entries.len());
        dict.entries.push((key, value));
    }
}

fn dict_remove(dict: &mut DictData, key: &DictKey) -> Option<Value> {
    let position = *dict.index.get(key)?;
    let (_, value) = dict.entries.remove(position);
    dict.index.remove(key);
    for (_, later) in dict.index.iter_mut() {
        if *later > position {
            *later -= 1;
        }
    }
    Some(value)
}

fn format_float(value: f64) -> String {
    if value.is_nan() {
        "nan".into()
    } else if value.is_infinite() {
        if value > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        }
    } else {
        format!("{value}")
    }
}

fn escape_repr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

enum Iterant {
    Items(std::vec::IntoIter<Value>),
    Range {
        next: i64,
        remaining: u64,
        step: i64,
    },
}

impl Iterant {
    fn next(&mut self) -> Option<Value> {
        match self {
            Iterant::Items(items) => items.next(),
            Iterant::Range {
                next,
                remaining,
                step,
            } => {
                if *remaining == 0 {
                    return None;
                }
                let current = *next;
                *next = current.checked_add(*step)?;
                *remaining -= 1;
                Some(Value::Int(current))
            }
        }
    }
}

fn generator_start(generators: &[ast::Comprehension]) -> ast::TextSize {
    generators
        .first()
        .map(|generator| generator.iter.start())
        .unwrap_or(ast::TextSize::default())
}

fn builtin_function(name: &str) -> Option<&'static str> {
    const BUILTINS: &[&str] = &[
        "len",
        "str",
        "int",
        "float",
        "bool",
        "abs",
        "min",
        "max",
        "sum",
        "round",
        "sorted",
        "reversed",
        "range",
        "enumerate",
        "zip",
        "list",
    ];
    BUILTINS
        .iter()
        .chain(api::HOST_FUNCTIONS.iter())
        .find(|candidate| **candidate == name)
        .copied()
}

// ── JSON conversion ────────────────────────────────────────────────────────

pub(crate) fn value_to_json(value: &Value, depth: usize) -> Option<serde_json::Value> {
    if depth > MAX_VALUE_DEPTH {
        return None;
    }
    match value {
        Value::None => Some(serde_json::Value::Null),
        Value::Bool(b) => Some((*b).into()),
        Value::Int(i) => Some((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f).map(serde_json::Value::Number),
        Value::Str(s) => {
            if s.len() > MAX_STRING_SIZE {
                None
            } else {
                Some(s.to_string().into())
            }
        }
        Value::List(list) => {
            let list = list.borrow();
            if list.items.len() > MAX_ARRAY_SIZE {
                return None;
            }
            let mut out = Vec::with_capacity(list.items.len());
            for item in &list.items {
                out.push(value_to_json(item, depth + 1)?);
            }
            Some(serde_json::Value::Array(out))
        }
        Value::Tuple(tuple) => {
            if tuple.items.len() > MAX_ARRAY_SIZE {
                return None;
            }
            let mut out = Vec::with_capacity(tuple.items.len());
            for item in &tuple.items {
                out.push(value_to_json(item, depth + 1)?);
            }
            Some(serde_json::Value::Array(out))
        }
        Value::Dict(dict) => {
            let dict = dict.borrow();
            if dict.entries.len() > MAX_MAP_SIZE {
                return None;
            }
            let mut out = serde_json::Map::new();
            for (key, value) in &dict.entries {
                let key = match key {
                    DictKey::Str(text) => text.to_string(),
                    DictKey::Int(i) => i.to_string(),
                    DictKey::Bool(b) => {
                        if *b {
                            "true".into()
                        } else {
                            "false".into()
                        }
                    }
                    DictKey::None => "null".into(),
                    DictKey::Tuple(_) => return None,
                };
                out.insert(key, value_to_json(value, depth + 1)?);
            }
            Some(serde_json::Value::Object(out))
        }
        Value::Range(_) | Value::Function(_) | Value::Builtin(_) | Value::Pair(_, _) => None,
    }
}

pub(crate) fn json_to_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::None,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(number) => number
            .as_i64()
            .map(Value::Int)
            .unwrap_or_else(|| Value::Float(number.as_f64().unwrap_or(f64::NAN))),
        serde_json::Value::String(text) => Value::Str(Rc::from(text.as_str())),
        serde_json::Value::Array(items) => Value::List(Rc::new(RefCell::new(ListData {
            items: items.iter().map(json_to_value).collect(),
            depth: 1,
        }))),
        serde_json::Value::Object(fields) => {
            let mut dict = DictData {
                entries: Vec::with_capacity(fields.len()),
                index: HashMap::new(),
                depth: 1,
            };
            for (key, value) in fields {
                dict_insert(
                    &mut dict,
                    DictKey::Str(Rc::from(key.as_str())),
                    json_to_value(value),
                );
            }
            Value::Dict(Rc::new(RefCell::new(dict)))
        }
    }
}
