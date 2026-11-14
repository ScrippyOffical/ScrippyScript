//! Lowers the parsed program to C and wraps it in the small runtime from
//! `runtime.c`. The system C compiler does the actual machine-code work.

use std::collections::HashMap;
use std::fmt::Write as _;

use scrippy_script_core::{
    AssignOp, BinOp, BindKind, Block, Expr, MapEntry, Param, Script, Span, Stmt, StrPart, UnOp,
};

const RUNTIME: &str = include_str!("runtime.c");

pub struct CompileError {
    pub span: Span,
    pub message: String,
}

impl CompileError {
    fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }

    pub fn render(&self, src: &str) -> String {
        let (line, column) = scrippy_script_core::span::line_col(src, self.span.start);
        let text = src.lines().nth(line - 1).unwrap_or("");
        let pad: String = text
            .chars()
            .take(column.saturating_sub(1))
            .map(|c| if c == '\t' { '\t' } else { ' ' })
            .collect();
        let width = src
            .get(self.span.range())
            .map_or(1, |slice| slice.chars().take_while(|c| *c != '\n').count())
            .max(1);
        format!(
            "{line}:{column}: {}\n  {text}\n  {pad}{}",
            self.message,
            "^".repeat(width)
        )
    }
}

pub fn generate(script: &Script, src: &str, file: &str) -> Result<String, CompileError> {
    let mut collector = Collector::default();
    for stmt in &script.stmts {
        collector.stmt(stmt);
    }

    let mut compiler = Compiler {
        src,
        out: String::new(),
        funcs: String::new(),
        defs: collector.defs,
        ids: collector.ids,
        contexts: Vec::new(),
        loops: Vec::new(),
        next_temp: 0,
        next_label: 0,
        indent: 1,
    };
    compiler.generate(script, file)
}

#[derive(Clone)]
struct FuncDef<'a> {
    id: usize,
    name: String,
    params: &'a [Param],
    body: &'a Block,
}

#[derive(Default)]
struct Collector<'a> {
    defs: Vec<FuncDef<'a>>,
    ids: HashMap<usize, usize>,
}

impl<'a> Collector<'a> {
    fn add(&mut self, name: &str, params: &'a [Param], body: &'a Block) {
        let id = self.defs.len();
        self.defs.push(FuncDef {
            id,
            name: name.to_string(),
            params,
            body,
        });
        self.ids.insert(body as *const Block as usize, id);
    }

    fn stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Fn {
                name, params, body, ..
            } => {
                self.add(name, params, body);
                self.defaults(params);
                self.stmt_list(&body.stmts);
            }
            Stmt::Bind { value, .. } => self.expr(value),
            Stmt::Assign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.expr(value);
                }
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                self.expr(cond);
                self.stmt_list(&then_branch.stmts);
                if let Some(other) = else_branch {
                    self.stmt(other);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond);
                self.stmt_list(&body.stmts);
            }
            Stmt::For { iter, body, .. } => {
                self.expr(iter);
                self.stmt_list(&body.stmts);
            }
            Stmt::Block(block) => self.stmt_list(&block.stmts),
            Stmt::Expr(expr) => self.expr(expr),
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }

    fn stmt_list(&mut self, stmts: &'a [Stmt]) {
        for stmt in stmts {
            self.stmt(stmt);
        }
    }

    fn expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Func { params, body, .. } => {
                self.add("<anonymous>", params, body);
                self.defaults(params);
                self.stmt_list(&body.stmts);
            }
            Expr::Str(lit) => {
                for part in &lit.parts {
                    if let StrPart::Expr(expr) = part {
                        self.expr(expr);
                    }
                }
            }
            Expr::List { items, .. } => {
                for item in items {
                    self.expr(item);
                }
            }
            Expr::Map { entries, .. } => {
                for MapEntry { value, .. } in entries {
                    self.expr(value);
                }
            }
            Expr::Call { callee, args, .. } => {
                self.expr(callee);
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::Member { object, .. } => self.expr(object),
            Expr::Index { object, index, .. } => {
                self.expr(object);
                self.expr(index);
            }
            Expr::New { args, .. } => {
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::Unary { expr, .. } => self.expr(expr),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
                ..
            } => {
                self.expr(cond);
                self.expr(then_expr);
                self.expr(else_expr);
            }
            Expr::Int { .. }
            | Expr::Float { .. }
            | Expr::Bool { .. }
            | Expr::Null { .. }
            | Expr::Ident { .. } => {}
        }
    }

    fn defaults(&mut self, params: &'a [Param]) {
        for param in params {
            if let Some(default) = &param.default {
                self.expr(default);
            }
        }
    }
}

#[derive(Default)]
struct Scope {
    vars: HashMap<String, Var>,
}

struct Var {
    slot: usize,
    mutable: bool,
}

struct FuncContext {
    depth: usize,
    scopes: Vec<Scope>,
    next_slot: usize,
    done_label: String,
    ret_var: String,
    name: String,
}

struct LoopInfo {
    break_label: String,
    continue_label: String,
    mark: String,
}

struct Compiler<'a> {
    src: &'a str,
    out: String,
    funcs: String,
    defs: Vec<FuncDef<'a>>,
    ids: HashMap<usize, usize>,
    contexts: Vec<FuncContext>,
    loops: Vec<LoopInfo>,
    next_temp: usize,
    next_label: usize,
    indent: usize,
}

impl<'a> Compiler<'a> {
    // A function context and a scope are pushed before any body is emitted, and
    // every function body is visited by the collector first. The expect calls
    // below check those invariants; a failing one is a compiler bug, not bad
    // input. Writing to a String cannot fail either.
    fn generate(&mut self, script: &'a Script, file: &str) -> Result<String, CompileError> {
        let mut context = FuncContext {
            depth: 0,
            scopes: vec![Scope::default()],
            next_slot: 0,
            done_label: "_done_main".to_string(),
            ret_var: "_ret".to_string(),
            name: "main".to_string(),
        };
        for stmt in &script.stmts {
            match stmt {
                Stmt::Bind { kind, name, .. } => {
                    let mutable = !matches!(kind, BindKind::Const);
                    if let Some(var) = context.scopes[0].vars.get_mut(name) {
                        var.mutable = mutable;
                    } else {
                        let slot = context.next_slot;
                        context.next_slot += 1;
                        context.scopes[0]
                            .vars
                            .insert(name.clone(), Var { slot, mutable });
                    }
                }
                Stmt::Fn { name, .. } if !context.scopes[0].vars.contains_key(name) => {
                    let slot = context.next_slot;
                    context.next_slot += 1;
                    context.scopes[0].vars.insert(
                        name.clone(),
                        Var {
                            slot,
                            mutable: false,
                        },
                    );
                }
                Stmt::Fn { .. } => {}
                _ => {}
            }
        }
        self.contexts.push(context);

        for stmt in &script.stmts {
            self.stmt(stmt, true)?;
        }

        let context = self.contexts.pop().expect("main context");
        let mut output = String::new();
        output.push_str(RUNTIME);
        output.push('\n');
        for def in &self.defs {
            writeln!(
                output,
                "Value fn_{}(Frame *closure, Value *args, int argc);",
                def.id
            )
            .expect("write to string");
        }
        output.push('\n');
        output.push_str(&self.funcs);

        writeln!(output, "int main(void) {{").expect("write to string");
        writeln!(output, "    rt_init({});", c_string(file.as_bytes())).expect("write");
        writeln!(
            output,
            "    Frame *fr = rt_frame(NULL, {}, \"<main>\");",
            context.next_slot
        )
        .expect("write");
        writeln!(output, "    rt_push_frame(fr);").expect("write");
        writeln!(output, "    size_t _base = rt_mark();").expect("write");
        writeln!(output, "    Value _ret = rt_null();").expect("write");
        output.push_str(&self.out);
        writeln!(output, "_done_main:").expect("write");
        writeln!(output, "    rt_reset(_base);").expect("write");
        writeln!(output, "    rt_pop_frame();").expect("write");
        writeln!(output, "    return 0;").expect("write");
        writeln!(output, "}}").expect("write");
        Ok(output)
    }

    fn stmt(&mut self, stmt: &'a Stmt, top_level: bool) -> Result<(), CompileError> {
        match stmt {
            Stmt::Block(block) => {
                let mark = self.start_stmt();
                self.block(block)?;
                self.end_stmt(&mark);
                Ok(())
            }
            Stmt::Break { span } => {
                let Some(loop_info) = self.loops.last() else {
                    return Err(CompileError::new(*span, "`break` outside a loop"));
                };
                let mark = loop_info.mark.clone();
                let label = loop_info.break_label.clone();
                self.emit(&format!("rt_reset({mark});"));
                self.emit(&format!("goto {label};"));
                Ok(())
            }
            Stmt::Continue { span } => {
                let Some(loop_info) = self.loops.last() else {
                    return Err(CompileError::new(*span, "`continue` outside a loop"));
                };
                let mark = loop_info.mark.clone();
                let label = loop_info.continue_label.clone();
                self.emit(&format!("rt_reset({mark});"));
                self.emit(&format!("goto {label};"));
                Ok(())
            }
            Stmt::Return { value, .. } => {
                let (ret_var, done) = {
                    let context = self.contexts.last().expect("function context");
                    (context.ret_var.clone(), context.done_label.clone())
                };
                let value = match value {
                    Some(expr) => self.expr(expr)?,
                    None => "rt_null()".to_string(),
                };
                self.emit(&format!("{ret_var} = {value};"));
                self.emit(&format!("goto {done};"));
                Ok(())
            }
            Stmt::Fn {
                name, body, span, ..
            } => {
                let id = *self
                    .ids
                    .get(&(body as *const Block as usize))
                    .expect("collected function");
                let def = self.defs[id].clone();
                let (slot, depth) = self.resolve(name).ok_or_else(|| {
                    CompileError::new(*span, format!("unknown function `{name}`"))
                })?;
                self.compile_function(&def)?;
                let mark = self.start_stmt();
                let frame = self.frame_ref(depth);
                self.emit(&format!(
                    "rt_slot_set({frame}, {slot}, rt_closure(fn_{id}, {}, {frame}));",
                    c_string(name.as_bytes())
                ));
                self.end_stmt(&mark);
                Ok(())
            }
            _ => {
                let mark = self.start_stmt();
                self.simple_stmt(stmt, top_level)?;
                self.end_stmt(&mark);
                Ok(())
            }
        }
    }

    fn simple_stmt(&mut self, stmt: &'a Stmt, top_level: bool) -> Result<(), CompileError> {
        match stmt {
            Stmt::Bind {
                kind,
                name,
                value,
                span,
                ..
            } => {
                let value = self.expr(value)?;
                let (slot, depth) = if top_level {
                    self.resolve(name).ok_or_else(|| {
                        CompileError::new(*span, format!("internal error: `{name}` not bound"))
                    })?
                } else {
                    let slot = self.declare(name, !matches!(kind, BindKind::Const))?;
                    (slot, 0)
                };
                let frame = self.frame_ref(depth);
                self.emit(&format!("rt_slot_set({frame}, {slot}, {value});"));
                Ok(())
            }
            Stmt::Assign {
                target,
                op,
                value,
                span,
            } => self.assign(target, *op, value, *span),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                self.emit("rt_maybe_gc();");
                let mark = self.fresh("_m");
                self.emit(&format!("size_t {mark} = rt_mark();"));
                let cond = self.expr(cond)?;
                let ok = self.fresh("_ok");
                self.emit(&format!("bool {ok} = rt_truthy({cond});"));
                self.emit(&format!("rt_reset({mark});"));
                self.emit(&format!("if ({ok})"));
                self.block(then_branch)?;
                if let Some(other) = else_branch {
                    self.emit("else {");
                    self.indent += 1;
                    self.stmt(other, false)?;
                    self.indent -= 1;
                    self.emit("}");
                }
                Ok(())
            }
            Stmt::While { cond, body, span } => {
                let _ = span;
                self.emit("rt_maybe_gc();");
                let mark = self.fresh("_loop");
                let break_label = self.fresh("_break");
                let continue_label = self.fresh("_continue");
                self.emit(&format!("size_t {mark} = rt_mark();"));
                self.emit("for (;;) {");
                self.indent += 1;
                self.emit("rt_maybe_gc();");
                let inner = self.fresh("_m");
                self.emit(&format!("size_t {inner} = rt_mark();"));
                let cond = self.expr(cond)?;
                let ok = self.fresh("_ok");
                self.emit(&format!("bool {ok} = rt_truthy({cond});"));
                self.emit(&format!("rt_reset({inner});"));
                self.emit(&format!("if (!{ok}) break;"));
                self.loops.push(LoopInfo {
                    break_label: break_label.clone(),
                    continue_label: continue_label.clone(),
                    mark: mark.clone(),
                });
                self.block(body)?;
                self.loops.pop();
                self.emit(&format!("{continue_label}: ;"));
                self.indent -= 1;
                self.emit("}");
                self.emit(&format!("{break_label}: ;"));
                self.emit(&format!("rt_reset({mark});"));
                Ok(())
            }
            Stmt::For {
                var,
                iter,
                body,
                span,
            } => {
                let _ = span;
                self.emit("rt_maybe_gc();");
                let mark = self.fresh("_loop");
                let break_label = self.fresh("_break");
                let continue_label = self.fresh("_continue");
                self.emit(&format!("size_t {mark} = rt_mark();"));
                let iter = self.expr(iter)?;
                let count = self.fresh("_n");
                self.emit(&format!(
                    "int64_t {count} = rt_iter_len({iter}, {});",
                    self.loc(*span)
                ));
                // The iterable lives in a temp until the loop ends, so continue
                // must not reset past it.
                let iter_mark = self.fresh("_iter");
                self.emit(&format!("size_t {iter_mark} = rt_mark();"));
                self.contexts
                    .last_mut()
                    .expect("function context")
                    .scopes
                    .push(Scope::default());
                let slot = self.declare(var, true)?;
                self.emit(&format!("for (int64_t _i = 0; _i < {count}; _i++) {{",));
                self.indent += 1;
                self.emit("rt_maybe_gc();");
                self.emit(&format!(
                    "rt_slot_set(fr, {slot}, rt_iter_at({iter}, _i, {}));",
                    self.loc(*span)
                ));
                self.loops.push(LoopInfo {
                    break_label: break_label.clone(),
                    continue_label: continue_label.clone(),
                    mark: iter_mark,
                });
                self.block(body)?;
                self.loops.pop();
                self.emit(&format!("{continue_label}: ;"));
                self.indent -= 1;
                self.emit("}");
                self.contexts
                    .last_mut()
                    .expect("function context")
                    .scopes
                    .pop();
                self.emit(&format!("{break_label}: ;"));
                self.emit(&format!("rt_reset({mark});"));
                Ok(())
            }
            Stmt::Expr(expr) => {
                self.expr(expr)?;
                Ok(())
            }
            _ => unreachable!("statement handled by caller"),
        }
    }

    fn assign(
        &mut self,
        target: &'a Expr,
        op: AssignOp,
        value: &'a Expr,
        span: Span,
    ) -> Result<(), CompileError> {
        let value = self.expr(value)?;
        let combined = match op {
            AssignOp::Assign => None,
            AssignOp::Add => Some(BinOp::Add),
            AssignOp::Sub => Some(BinOp::Sub),
            AssignOp::Mul => Some(BinOp::Mul),
            AssignOp::Div => Some(BinOp::Div),
            AssignOp::Rem => Some(BinOp::Rem),
        };

        match target {
            Expr::Ident { name, span } => {
                let Some((slot, depth, mutable)) = self.resolve_full(name) else {
                    return Err(CompileError::new(
                        *span,
                        format!("unknown variable `{name}`"),
                    ));
                };
                if !mutable {
                    return Err(CompileError::new(
                        *span,
                        format!("cannot assign to read-only `{name}`"),
                    ));
                }
                let frame = self.frame_ref(depth);
                let value = match combined {
                    None => value,
                    Some(op) => {
                        let current = self.fresh("_t");
                        self.emit(&format!(
                            "Value {current} = rt_slot_get({frame}, {slot}, {}, {});",
                            c_string(name.as_bytes()),
                            self.loc(*span)
                        ));
                        let result = self.fresh("_t");
                        self.emit(&format!(
                            "Value {result} = rt_binary({}, {current}, {value}, {});",
                            bin_code(op),
                            self.loc(*span)
                        ));
                        result
                    }
                };
                self.emit(&format!("rt_slot_set({frame}, {slot}, {value});"));
                Ok(())
            }
            Expr::Member {
                object, name, span, ..
            } => {
                let object = self.expr(object)?;
                let value = match combined {
                    None => value,
                    Some(op) => {
                        let current = self.fresh("_t");
                        self.emit(&format!(
                            "Value {current} = rt_member_get({object}, {}, {});",
                            c_string(name.as_bytes()),
                            self.loc(*span)
                        ));
                        let result = self.fresh("_t");
                        self.emit(&format!(
                            "Value {result} = rt_binary({}, {current}, {value}, {});",
                            bin_code(op),
                            self.loc(*span)
                        ));
                        result
                    }
                };
                self.emit(&format!(
                    "rt_member_set({object}, {}, {value}, {});",
                    c_string(name.as_bytes()),
                    self.loc(*span)
                ));
                Ok(())
            }
            Expr::Index {
                object,
                index,
                span,
            } => {
                let object = self.expr(object)?;
                let index = self.expr(index)?;
                let value = match combined {
                    None => value,
                    Some(op) => {
                        let current = self.fresh("_t");
                        self.emit(&format!(
                            "Value {current} = rt_index_get({object}, {index}, {});",
                            self.loc(*span)
                        ));
                        let result = self.fresh("_t");
                        self.emit(&format!(
                            "Value {result} = rt_binary({}, {current}, {value}, {});",
                            bin_code(op),
                            self.loc(*span)
                        ));
                        result
                    }
                };
                self.emit(&format!(
                    "rt_index_set({object}, {index}, {value}, {});",
                    self.loc(*span)
                ));
                Ok(())
            }
            _ => Err(CompileError::new(span, "cannot assign to this expression")),
        }
    }

    fn block(&mut self, block: &'a Block) -> Result<(), CompileError> {
        self.emit("{");
        self.indent += 1;
        self.push_scope();
        self.prebind_functions(&block.stmts);
        for stmt in &block.stmts {
            self.stmt(stmt, false)?;
        }
        self.pop_scope();
        self.indent -= 1;
        self.emit("}");
        Ok(())
    }

    fn prebind_functions(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            if let Stmt::Fn { name, .. } = stmt {
                if self
                    .contexts
                    .last()
                    .expect("context")
                    .scopes
                    .last()
                    .expect("scope")
                    .vars
                    .contains_key(name)
                {
                    continue;
                }
                let _ = self.declare(name, false);
            }
        }
    }

    fn compile_function(&mut self, def: &FuncDef<'a>) -> Result<(), CompileError> {
        let saved_out = std::mem::take(&mut self.out);
        let saved_indent = self.indent;
        let saved_loops = std::mem::take(&mut self.loops);
        self.indent = 0;

        let parent_depth = self.contexts.last().map_or(0, |context| context.depth);
        let suffix = self.next_label;
        self.next_label += 1;
        let context = FuncContext {
            depth: parent_depth + 1,
            scopes: Vec::new(),
            next_slot: 0,
            done_label: format!("_done_{suffix}"),
            ret_var: "_ret".to_string(),
            name: def.name.clone(),
        };
        self.contexts.push(context);
        self.push_scope();

        self.emit(&format!("if (argc > {}) {{", def.params.len()));
        self.indent += 1;
        self.emit(&format!(
            "rt_arity({}, {}, argc, {});",
            c_string(def.name.as_bytes()),
            def.params.len(),
            "rt_frame_loc(fr)"
        ));
        self.indent -= 1;
        self.emit("}");

        for (index, param) in def.params.iter().enumerate() {
            let slot = self.allocate_slot();
            self.emit(&format!("if (argc > {index}) {{"));
            self.indent += 1;
            self.emit(&format!("rt_slot_set(fr, {slot}, args[{index}]);"));
            self.indent -= 1;
            self.emit("} else {");
            self.indent += 1;
            if let Some(default) = &param.default {
                let value = self.expr(default)?;
                self.emit(&format!("rt_slot_set(fr, {slot}, {value});"));
            } else {
                self.emit(&format!(
                    "rt_missing({}, {}, {});",
                    c_string(def.name.as_bytes()),
                    c_string(param.name.as_bytes()),
                    "rt_frame_loc(fr)"
                ));
            }
            self.indent -= 1;
            self.emit("}");
            self.contexts
                .last_mut()
                .expect("function context")
                .scopes
                .last_mut()
                .expect("scope")
                .vars
                .insert(
                    param.name.clone(),
                    Var {
                        slot,
                        mutable: false,
                    },
                );
        }

        let args_slot = self.declare("args", false)?;
        self.emit(&format!(
            "rt_slot_set(fr, {args_slot}, rt_args_list(args, argc));"
        ));

        self.block(def.body)?;
        self.pop_scope();

        let context = self.contexts.pop().expect("function context");
        let body = std::mem::take(&mut self.out);
        let definition = format!(
            concat!(
                "Value fn_{}(Frame *closure, Value *args, int argc) {{\n",
                "    Frame *fr = rt_frame(closure, {}, {});\n",
                "    rt_push_frame(fr);\n",
                "    size_t _base = rt_mark();\n",
                "    Value _ret = rt_null();\n",
                "{}\n",
                "    {}:\n",
                "    rt_reset(_base);\n",
                "    rt_pop_frame();\n",
                "    return _ret;\n",
                "}}\n\n",
            ),
            def.id,
            context.next_slot,
            c_string(context.name.as_bytes()),
            indent_text(&body, 1),
            context.done_label
        );
        self.funcs.push_str(&definition);

        self.out = saved_out;
        self.indent = saved_indent;
        self.loops = saved_loops;
        Ok(())
    }

    fn expr(&mut self, expr: &'a Expr) -> Result<String, CompileError> {
        let temp = self.fresh("_t");
        match expr {
            Expr::Int { value, .. } => {
                self.emit(&format!("Value {temp} = rt_int({});", c_int(*value)));
            }
            Expr::Float { value, .. } => {
                self.emit(&format!("Value {temp} = rt_float({});", c_float(*value)));
            }
            Expr::Bool { value, .. } => {
                self.emit(&format!(
                    "Value {temp} = rt_bool({});",
                    if *value { "true" } else { "false" }
                ));
            }
            Expr::Null { .. } => {
                self.emit(&format!("Value {temp} = rt_null();"));
            }
            Expr::Str(lit) => {
                self.emit(&format!("Value {temp} = rt_str(\"\", 0);"));
                for part in &lit.parts {
                    match part {
                        StrPart::Text(text) => {
                            let piece = self.fresh("_t");
                            self.emit(&format!(
                                "Value {piece} = rt_str({}, {});",
                                c_string(text.as_bytes()),
                                text.len()
                            ));
                            self.emit(&format!(
                                "{temp} = rt_binary(OP_ADD, {temp}, {piece}, {});",
                                self.loc(lit.span)
                            ));
                        }
                        StrPart::Expr(expr) => {
                            let value = self.expr(expr)?;
                            self.emit(&format!(
                                "{temp} = rt_binary(OP_ADD, {temp}, {value}, {});",
                                self.loc(lit.span)
                            ));
                        }
                    }
                }
            }
            Expr::Ident { name, span } => {
                if let Some((slot, depth, _)) = self.resolve_full(name) {
                    let frame = self.frame_ref(depth);
                    self.emit(&format!(
                        "Value {temp} = rt_slot_get({frame}, {slot}, {}, {});",
                        c_string(name.as_bytes()),
                        self.loc(*span)
                    ));
                } else if is_builtin(name) {
                    self.emit(&format!(
                        "Value {temp} = rt_builtin({});",
                        c_string(name.as_bytes())
                    ));
                } else {
                    return Err(CompileError::new(
                        *span,
                        format!("unknown variable or function `{name}`"),
                    ));
                }
            }
            Expr::List { items, .. } => {
                self.emit(&format!("Value {temp} = rt_list();"));
                for item in items {
                    let value = self.expr(item)?;
                    self.emit(&format!("rt_list_push_lit({temp}, {value});"));
                }
            }
            Expr::Map { entries, .. } => {
                self.emit(&format!("Value {temp} = rt_map();"));
                for entry in entries {
                    let value = self.expr(&entry.value)?;
                    self.emit(&format!(
                        "rt_map_put_lit({temp}, {}, {}, {value});",
                        c_string(entry.key.as_bytes()),
                        entry.key.len()
                    ));
                }
            }
            Expr::Call { callee, args, span } => {
                if let Expr::Member {
                    object,
                    name,
                    safe,
                    span: member_span,
                } = callee.as_ref()
                {
                    let object = self.expr(object)?;
                    if *safe {
                        self.emit(&format!("Value {temp};"));
                        self.emit(&format!("if ({object}.type == T_NULL) {{"));
                        self.indent += 1;
                        self.emit(&format!("{temp} = rt_null();"));
                        self.indent -= 1;
                        self.emit("} else {");
                        self.indent += 1;
                        let call = self.method_call(&object, name, args, *span)?;
                        self.emit(&format!("{temp} = {call};"));
                        self.indent -= 1;
                        self.emit("}");
                    } else {
                        let call = self.method_call(&object, name, args, *span)?;
                        self.emit(&format!("Value {temp} = {call};"));
                    }
                    let _ = member_span;
                } else {
                    let callee = self.expr(callee)?;
                    let (array, count) = self.arguments(args)?;
                    self.emit(&format!(
                        "Value {temp} = rt_call({callee}, {array}, {count}, {});",
                        self.loc(*span)
                    ));
                    self.emit(&format!("rt_hold({temp});"));
                }
            }
            Expr::Member {
                object,
                name,
                safe,
                span,
            } => {
                let object = self.expr(object)?;
                if *safe {
                    self.emit(&format!("Value {temp};"));
                    self.emit(&format!("if ({object}.type == T_NULL) {{"));
                    self.indent += 1;
                    self.emit(&format!("{temp} = rt_null();"));
                    self.indent -= 1;
                    self.emit("} else {");
                    self.indent += 1;
                    self.emit(&format!(
                        "{temp} = rt_member_get({object}, {}, {});",
                        c_string(name.as_bytes()),
                        self.loc(*span)
                    ));
                    self.indent -= 1;
                    self.emit("}");
                } else {
                    self.emit(&format!(
                        "Value {temp} = rt_member_get({object}, {}, {});",
                        c_string(name.as_bytes()),
                        self.loc(*span)
                    ));
                }
            }
            Expr::Index {
                object,
                index,
                span,
            } => {
                let object = self.expr(object)?;
                let index = self.expr(index)?;
                self.emit(&format!(
                    "Value {temp} = rt_index_get({object}, {index}, {});",
                    self.loc(*span)
                ));
            }
            Expr::New { span, .. } => {
                return Err(CompileError::new(
                    *span,
                    "`new` needs a host; the native backend has no Java objects",
                ));
            }
            Expr::Func { body, span, .. } => {
                let id = *self
                    .ids
                    .get(&(body as *const Block as usize))
                    .expect("collected function");
                let def = self.defs[id].clone();
                self.compile_function(&def)?;
                self.emit(&format!(
                    "Value {temp} = rt_closure(fn_{id}, \"<anonymous>\", fr);"
                ));
                let _ = span;
            }
            Expr::Unary { op, expr, span } => {
                let value = self.expr(expr)?;
                match op {
                    UnOp::Neg => self.emit(&format!(
                        "Value {temp} = rt_neg({value}, {});",
                        self.loc(*span)
                    )),
                    UnOp::Not => self.emit(&format!("Value {temp} = rt_not({value});")),
                }
            }
            Expr::Binary { op, lhs, rhs, span } => match op {
                BinOp::And | BinOp::Or => {
                    let left = self.expr(lhs)?;
                    self.emit(&format!("Value {temp} = {left};"));
                    let check = if matches!(op, BinOp::And) {
                        format!("rt_truthy({temp})")
                    } else {
                        format!("!rt_truthy({temp})")
                    };
                    self.emit(&format!("if ({check}) {{"));
                    self.indent += 1;
                    let right = self.expr(rhs)?;
                    self.emit(&format!("{temp} = {right};"));
                    self.indent -= 1;
                    self.emit("}");
                }
                BinOp::Coalesce => {
                    let left = self.expr(lhs)?;
                    self.emit(&format!("Value {temp} = {left};"));
                    self.emit(&format!("if ({temp}.type == T_NULL) {{"));
                    self.indent += 1;
                    let right = self.expr(rhs)?;
                    self.emit(&format!("{temp} = {right};"));
                    self.indent -= 1;
                    self.emit("}");
                }
                _ => {
                    let left = self.expr(lhs)?;
                    let right = self.expr(rhs)?;
                    self.emit(&format!(
                        "Value {temp} = rt_binary({}, {left}, {right}, {});",
                        bin_code(*op),
                        self.loc(*span)
                    ));
                }
            },
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
                span,
            } => {
                let cond = self.expr(cond)?;
                self.emit(&format!("Value {temp};"));
                self.emit(&format!("if (rt_truthy({cond})) {{"));
                self.indent += 1;
                let value = self.expr(then_expr)?;
                self.emit(&format!("{temp} = {value};"));
                self.indent -= 1;
                self.emit("} else {");
                self.indent += 1;
                let value = self.expr(else_expr)?;
                self.emit(&format!("{temp} = {value};"));
                self.indent -= 1;
                self.emit("}");
                let _ = span;
            }
        }
        Ok(temp)
    }

    fn method_call(
        &mut self,
        object: &str,
        name: &str,
        args: &'a [Expr],
        span: Span,
    ) -> Result<String, CompileError> {
        let (array, count) = self.arguments(args)?;
        let temp = self.fresh("_t");
        self.emit(&format!(
            "Value {temp} = rt_method_call({object}, {}, {array}, {count}, {});",
            c_string(name.as_bytes()),
            self.loc(span)
        ));
        self.emit(&format!("rt_hold({temp});"));
        Ok(temp)
    }

    fn arguments(&mut self, args: &'a [Expr]) -> Result<(String, usize), CompileError> {
        if args.is_empty() {
            return Ok(("NULL".to_string(), 0));
        }
        let array = self.fresh("_args");
        self.emit(&format!("Value {array}[{}];", args.len()));
        for (index, arg) in args.iter().enumerate() {
            let value = self.expr(arg)?;
            self.emit(&format!("{array}[{index}] = {value};"));
        }
        Ok((array, args.len()))
    }

    fn declare(&mut self, name: &str, mutable: bool) -> Result<usize, CompileError> {
        let slot = self.allocate_slot();
        let scope = self
            .contexts
            .last_mut()
            .expect("function context")
            .scopes
            .last_mut()
            .expect("scope");
        scope.vars.insert(name.to_string(), Var { slot, mutable });
        Ok(slot)
    }

    fn allocate_slot(&mut self) -> usize {
        let context = self.contexts.last_mut().expect("function context");
        let slot = context.next_slot;
        context.next_slot += 1;
        slot
    }

    fn push_scope(&mut self) {
        self.contexts
            .last_mut()
            .expect("function context")
            .scopes
            .push(Scope::default());
    }

    fn pop_scope(&mut self) {
        self.contexts
            .last_mut()
            .expect("function context")
            .scopes
            .pop();
    }

    fn resolve(&self, name: &str) -> Option<(usize, usize)> {
        self.resolve_full(name)
            .map(|(slot, depth, _)| (slot, depth))
    }

    fn resolve_full(&self, name: &str) -> Option<(usize, usize, bool)> {
        let current = self.contexts.last()?.depth;
        for context in self.contexts.iter().rev() {
            for scope in context.scopes.iter().rev() {
                if let Some(var) = scope.vars.get(name) {
                    return Some((var.slot, current - context.depth, var.mutable));
                }
            }
        }
        None
    }

    fn frame_ref(&self, depth: usize) -> String {
        if depth == 0 {
            "fr".to_string()
        } else {
            format!("rt_ancestor(fr, {depth})")
        }
    }

    fn start_stmt(&mut self) -> String {
        self.emit("rt_maybe_gc();");
        let mark = self.fresh("_m");
        self.emit(&format!("size_t {mark} = rt_mark();"));
        mark
    }

    fn end_stmt(&mut self, mark: &str) {
        self.emit(&format!("rt_reset({mark});"));
    }

    fn fresh(&mut self, prefix: &str) -> String {
        let id = self.next_temp;
        self.next_temp += 1;
        format!("{prefix}{id}")
    }

    fn loc(&self, span: Span) -> String {
        let (line, column) = scrippy_script_core::span::line_col(self.src, span.start);
        format!("(Loc){{{line}, {column}}}")
    }

    fn emit(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }
}

fn indent_text(text: &str, levels: usize) -> String {
    let pad = "    ".repeat(levels);
    let mut output = String::new();
    for line in text.lines() {
        if line.is_empty() {
            output.push('\n');
        } else {
            output.push_str(&pad);
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

fn bin_code(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "OP_ADD",
        BinOp::Sub => "OP_SUB",
        BinOp::Mul => "OP_MUL",
        BinOp::Div => "OP_DIV",
        BinOp::Rem => "OP_REM",
        BinOp::Eq => "OP_EQ",
        BinOp::Ne => "OP_NE",
        BinOp::Lt => "OP_LT",
        BinOp::Le => "OP_LE",
        BinOp::Gt => "OP_GT",
        BinOp::Ge => "OP_GE",
        BinOp::Range => "OP_RANGE",
        BinOp::RangeExcl => "OP_RANGE_EXCL",
        BinOp::And | BinOp::Or | BinOp::Coalesce => {
            unreachable!("short-circuit operators are lowered separately")
        }
    }
}

fn c_int(value: i64) -> String {
    match value {
        i64::MIN => "INT64_MIN".to_string(),
        value if value < 0 => format!("(-INT64_C({}))", -(value as i128)),
        value => format!("INT64_C({value})"),
    }
}

fn c_float(value: f64) -> String {
    if value.is_nan() {
        return "NAN".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 {
            "-INFINITY".to_string()
        } else {
            "INFINITY".to_string()
        };
    }
    let text = format!("{value:?}");
    if text.contains('.') || text.contains('e') || text.contains('E') {
        text
    } else {
        format!("{text}.0")
    }
}

fn c_string(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    for &byte in bytes {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(byte as char),
            _ => {
                let _ = write!(out, "\\{byte:03o}");
            }
        }
    }
    out.push('"');
    out
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "log"
            | "str"
            | "toString"
            | "num"
            | "int"
            | "bool"
            | "typeof"
            | "isNull"
            | "len"
            | "size"
            | "list"
            | "keys"
            | "values"
            | "contains"
            | "add"
            | "remove"
            | "sort"
            | "reverse"
            | "join"
            | "split"
            | "range"
            | "upper"
            | "lower"
            | "trim"
            | "replace"
            | "startsWith"
            | "endsWith"
            | "substring"
            | "color"
            | "colorize"
            | "abs"
            | "floor"
            | "ceil"
            | "round"
            | "sqrt"
            | "pow"
            | "min"
            | "max"
            | "random"
            | "randomInt"
            | "now"
            | "uuid"
    )
}
