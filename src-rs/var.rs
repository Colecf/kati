/*
Copyright 2025 Google LLC

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

     https://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashMap, HashSet},
    ffi::OsString,
    fmt::Debug,
    os::unix::ffi::OsStrExt,
    rc::Rc,
};

use anyhow::Result;
use bytes::{BufMut, Bytes};
use parking_lot::RwLock;

use crate::{
    command::AutoCommandVar,
    error, error_loc,
    eval::Frame,
    loc::Loc,
    strutil::{WordWriter, has_path_prefix},
    symtab::get_symbol_names,
    warn_loc,
};
use crate::{
    eval::Evaluator,
    expr::{Evaluable, Value},
    stmt::AssignOp,
    symtab::Symbol,
};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum VarOrigin {
    Default,
    Environment,
    EnvironmentOverride,
    File,
    CommandLine,
    Override,
    Automatic,
}

pub fn get_origin_str(origin: VarOrigin) -> &'static str {
    match origin {
        VarOrigin::Default => "default",
        VarOrigin::Environment => "environment",
        VarOrigin::EnvironmentOverride => "environment override",
        VarOrigin::File => "file",
        VarOrigin::CommandLine => "command line",
        VarOrigin::Override => "override",
        VarOrigin::Automatic => "automatic",
    }
}

pub type Var = Rc<RwLock<Variable>>;

#[derive(Debug)]
pub struct Variable {
    loc: Option<Loc>,

    definition: Option<Rc<Frame>>,

    origin: VarOrigin,

    pub assign_op: Option<AssignOp>,
    pub readonly: bool,
    pub deprecated: Option<Rc<String>>,
    obsolete: Option<Rc<String>>,

    visibility_prefix: Option<Vec<OsString>>,

    value: InnerVar,
}

#[derive(Debug)]
pub enum InnerVar {
    Simple(Vec<u8>),
    Recursive { v: Rc<Value>, orig: Bytes },
    AutoCommand(Symbol, AutoCommandVar),
    ShellStatus,
    VariableNames { name: Bytes, all: bool },
}

impl Variable {
    pub fn loc(&self) -> &Option<Loc> {
        &self.loc
    }
    pub fn origin(&self) -> VarOrigin {
        self.origin
    }
    pub fn definition(&self) -> &Option<Rc<Frame>> {
        &self.definition
    }
    pub fn obsolete(&self) -> bool {
        self.obsolete.is_some()
    }
    pub fn set_obsolete(&mut self, message: Rc<String>) {
        self.obsolete = Some(message);
    }
    pub fn flavor(&self) -> &'static str {
        match &self.value {
            InnerVar::Simple(_) => "simple",
            InnerVar::Recursive { .. } => "recursive",
            InnerVar::AutoCommand(_, _) => "undefined",
            InnerVar::ShellStatus => "simple",
            InnerVar::VariableNames { .. } => "kati_variable_names",
        }
    }
    pub fn used(&self, ev: &Evaluator, sym: &Symbol) -> Result<()> {
        if let Some(obsolete) = &self.obsolete {
            error_loc!(ev.loc.as_ref(), "*** {sym} is obsolete{obsolete}.");
        }
        if let Some(deprecated) = &self.deprecated {
            warn_loc!(ev.loc.as_ref(), "{sym} has been deprecated{deprecated}.");
        }
        Ok(())
    }
    pub fn set_visibility_prefix(&mut self, prefixes: Vec<OsString>, name: &Symbol) -> Result<()> {
        if self.visibility_prefix.is_none() {
            self.visibility_prefix = Some(prefixes);
        } else if self.visibility_prefix != Some(prefixes) {
            error!("Visibility prefix conflict on variable: {name}");
        }
        Ok(())
    }
    pub fn immediate_eval(&self) -> bool {
        matches!(&self.value, InnerVar::Simple(_))
    }
    pub fn append_var(&mut self, v: Rc<Value>, frame: Rc<Frame>, loc: Option<&Loc>) -> Result<()> {
        match &mut self.value {
            InnerVar::Simple(_) => {
                panic!("append_var should not be used when immediate_eval returns true")
            }
            InnerVar::Recursive { v: prev, .. } => {
                *prev = Rc::new(Value::List(
                    prev.loc(),
                    vec![
                        prev.clone(),
                        Rc::new(Value::Literal(None, Bytes::from_static(b" "))),
                        v,
                    ],
                ));
                self.definition = Some(frame);
            }
            InnerVar::AutoCommand(sym, _) => {
                error_loc!(loc, "appending to ${sym} is not supported");
            }
            InnerVar::ShellStatus => panic!(),
            InnerVar::VariableNames { .. } => panic!(),
        }
        Ok(())
    }
    pub fn append_str(&mut self, buf: &Bytes, frame: Rc<Frame>) -> Result<()> {
        match &mut self.value {
            InnerVar::Simple(s) => {
                s.push(b' ');
                s.extend_from_slice(buf);
                self.definition = Some(frame);
            }
            InnerVar::Recursive { v: prev, .. } => {
                *prev = Rc::new(Value::List(
                    prev.loc(),
                    vec![
                        prev.clone(),
                        Rc::new(Value::Literal(None, Bytes::from_static(b" "))),
                        Rc::new(Value::Literal(None, buf.clone())),
                    ],
                ));
                self.definition = Some(frame);
            }
            InnerVar::AutoCommand(sym, _) => {
                error!("appending to ${sym} is not supported");
            }
            InnerVar::ShellStatus => panic!(),
            InnerVar::VariableNames { .. } => panic!(),
        }
        Ok(())
    }
    pub fn check_current_referencing_file(&self, loc: &Option<Loc>, sym: Symbol) -> Result<()> {
        let Some(prefixes) = &self.visibility_prefix else {
            return Ok(());
        };
        let loc = loc.clone().unwrap_or_default();
        let mut valid = false;
        for prefix in prefixes {
            if has_path_prefix(&loc.filename.as_bytes(), prefix.as_bytes()) {
                valid = true;
                break;
            }
        }
        if !valid {
            let s = prefixes
                .iter()
                .map(|s| s.to_string_lossy())
                .collect::<Vec<Cow<str>>>()
                .join("\n");
            error!(
                "{} is not a valid file to reference variable {sym}. Line #{}.\nValid file prefixes:\n{s}",
                loc.filename, loc.line
            );
        }
        Ok(())
    }
    pub fn string(&self) -> Result<Cow<'_, [u8]>> {
        Ok(match &self.value {
            InnerVar::Simple(s) => Cow::Borrowed(s.as_slice()),
            InnerVar::Recursive { v: _, orig } => Cow::Borrowed(orig),
            InnerVar::AutoCommand(sym, _) => {
                error!("$(value {sym}) is not implemented yet");
            }
            InnerVar::ShellStatus => SHELL_STATUS.with_borrow_mut(|status| {
                Cow::Owned(if let Some(status) = status.as_ref() {
                    status.to_string().as_bytes().to_vec()
                } else {
                    Vec::new()
                })
            }),
            InnerVar::VariableNames { name, .. } => Cow::Borrowed(name),
        })
    }

    pub fn new_simple(
        origin: VarOrigin,
        frame: Option<Rc<Frame>>,
        loc: Option<Loc>,
    ) -> Rc<RwLock<Self>> {
        Rc::new(RwLock::new(Self {
            loc,
            definition: frame,
            origin,
            assign_op: None,
            readonly: false,
            deprecated: None,
            obsolete: None,
            visibility_prefix: None,
            value: InnerVar::Simple(Vec::new()),
        }))
    }

    pub fn with_simple_string(
        value: Bytes,
        origin: VarOrigin,
        frame: Option<Rc<Frame>>,
        loc: Option<Loc>,
    ) -> Rc<RwLock<Self>> {
        Rc::new(RwLock::new(Self {
            loc,
            definition: frame,
            origin,
            assign_op: None,
            readonly: false,
            deprecated: None,
            obsolete: None,
            visibility_prefix: None,
            value: InnerVar::Simple(value.to_vec()),
        }))
    }

    pub fn with_simple_value(
        origin: VarOrigin,
        frame: Option<Rc<Frame>>,
        loc: Option<Loc>,
        ev: &mut Evaluator,
        v: &Value,
    ) -> Result<Rc<RwLock<Self>>> {
        let value = v.eval_to_buf(ev)?;
        Ok(Rc::new(RwLock::new(Self {
            loc,
            definition: frame,
            origin,
            assign_op: None,
            readonly: false,
            deprecated: None,
            obsolete: None,
            visibility_prefix: None,
            value: InnerVar::Simple(value.to_vec()),
        })))
    }

    pub fn new_recursive(
        v: Rc<Value>,
        origin: VarOrigin,
        frame: Option<Rc<Frame>>,
        loc: Option<Loc>,
        orig: Bytes,
    ) -> Rc<RwLock<Self>> {
        Rc::new(RwLock::new(Self {
            loc,
            definition: frame,
            origin,
            assign_op: None,
            readonly: false,
            deprecated: None,
            obsolete: None,
            visibility_prefix: None,
            value: InnerVar::Recursive { v, orig },
        }))
    }

    pub fn new_autocommand(sym: Symbol, a: AutoCommandVar) -> Rc<RwLock<Self>> {
        Rc::new(RwLock::new(Self {
            loc: None,
            definition: None,
            origin: VarOrigin::Automatic,
            assign_op: None,
            readonly: false,
            deprecated: None,
            obsolete: None,
            visibility_prefix: None,
            value: InnerVar::AutoCommand(sym, a),
        }))
    }

    pub fn new_shell_status_var() -> Rc<RwLock<Self>> {
        Rc::new(RwLock::new(Self {
            loc: None,
            definition: None,
            origin: VarOrigin::Override,
            assign_op: Some(AssignOp::ColonEq),
            readonly: true,
            deprecated: None,
            obsolete: None,
            visibility_prefix: None,
            value: InnerVar::ShellStatus,
        }))
    }

    pub fn new_variable_names(name: &'static [u8], all: bool) -> Rc<RwLock<Self>> {
        Rc::new(RwLock::new(Self {
            loc: None,
            definition: None,
            origin: VarOrigin::Override,
            assign_op: Some(AssignOp::ColonEq),
            readonly: true,
            deprecated: None,
            obsolete: None,
            visibility_prefix: None,
            value: InnerVar::VariableNames {
                name: Bytes::from_static(name),
                all,
            },
        }))
    }
}

impl Evaluable for Variable {
    fn eval(&self, ev: &mut crate::eval::Evaluator, out: &mut dyn BufMut) -> Result<()> {
        match &self.value {
            InnerVar::Simple(v) => {
                out.put_slice(v);
            }
            InnerVar::Recursive { v, .. } => {
                v.eval(ev, out)?;
            }
            InnerVar::AutoCommand(_, a) => {
                a.eval(ev, out)?;
            }
            InnerVar::ShellStatus => {
                if ev.is_evaluating_command {
                    error_loc!(
                        ev.loc.as_ref(),
                        "Kati does not support using .SHELLSTATUS inside of a rule"
                    );
                }

                SHELL_STATUS.with_borrow_mut(|status| {
                    if let Some(status) = status.as_ref() {
                        out.put_slice(format!("{status}").as_bytes());
                    }
                })
            }
            InnerVar::VariableNames { all, .. } => {
                let mut ww = WordWriter::new(out);
                let symbols = get_symbol_names(|var| {
                    if var.read().obsolete() {
                        return false;
                    }
                    true
                });
                for (sym, entry) in symbols {
                    if !*all
                        && let Some(var) = sym.peek_global_var()
                        && var.read().is_func()
                    {
                        continue;
                    }
                    ww.write(&entry);
                }
            }
        }
        Ok(())
    }
    fn is_func(&self) -> bool {
        match &self.value {
            InnerVar::Simple(_) => false,
            InnerVar::Recursive { v, .. } => v.is_func(),
            InnerVar::AutoCommand(_, _) => true,
            InnerVar::ShellStatus => false,
            InnerVar::VariableNames { .. } => false,
        }
    }
}

thread_local! {
    static SHELL_STATUS: RefCell<Option<i32>> = RefCell::new(None);
}

pub fn set_shell_status_var(status: i32) {
    SHELL_STATUS.with_borrow_mut(|s| *s = Some(status))
}

thread_local! {
    pub static USED_ENV_VARS: RefCell<HashSet<Symbol>> = RefCell::new(HashSet::new());
}

pub struct Vars(pub RefCell<HashMap<Symbol, Var>>);

impl Default for Vars {
    fn default() -> Self {
        Self::new()
    }
}

impl Vars {
    pub fn new() -> Self {
        Vars(RefCell::new(HashMap::new()))
    }

    pub fn lookup(&self, sym: Symbol) -> Option<Var> {
        let ret = self.0.borrow_mut().get(&sym).cloned()?;
        match ret.read().origin() {
            VarOrigin::Environment | VarOrigin::EnvironmentOverride => {
                USED_ENV_VARS.with_borrow_mut(|vars| vars.insert(sym));
            }
            _ => {}
        }
        Some(ret)
    }

    pub fn peek(&self, sym: Symbol) -> Option<Var> {
        self.0.borrow_mut().get(&sym).cloned()
    }

    pub fn assign(&self, sym: Symbol, var: Var, readonly: &mut bool) -> Result<()> {
        *readonly = false;
        let mut vars = self.0.borrow_mut();
        if let Some(orig) = vars.get_mut(&sym) {
            if orig.read().readonly {
                *readonly = true;
                return Ok(());
            }
            match orig.read().origin() {
                VarOrigin::Override | VarOrigin::EnvironmentOverride => return Ok(()),
                VarOrigin::Automatic => {
                    error!("overriding automatic variable is not implemented yet");
                }
                _ => {}
            }
            *orig = var;
        } else {
            vars.insert(sym, var);
        }
        Ok(())
    }

    pub fn merge_from(&self, vars: &Vars) {
        let mut to = self.0.borrow_mut();
        let from = vars.0.borrow_mut();

        for (sym, var) in from.iter() {
            to.insert(*sym, var.clone());
        }
    }
}

impl Clone for Vars {
    fn clone(&self) -> Self {
        let m = self.0.borrow_mut();
        Self(RefCell::new(m.clone()))
    }
}

impl Debug for Vars {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let vars = self.0.borrow_mut();
        f.debug_map().entries(vars.iter()).finish()
    }
}

pub struct ScopedVar {
    vars: Rc<Vars>,
    sym: Symbol,
    orig: Option<Var>,
}

impl ScopedVar {
    pub fn new(vars: Rc<Vars>, sym: Symbol, var: Var) -> Self {
        let orig = {
            let mut vars = vars.0.borrow_mut();
            vars.insert(sym, var)
        };
        Self { vars, sym, orig }
    }
}

impl Drop for ScopedVar {
    fn drop(&mut self) {
        let mut vars = self.vars.0.borrow_mut();
        if let Some(orig) = self.orig.clone() {
            vars.insert(self.sym, orig);
        } else {
            vars.remove(&self.sym);
        }
    }
}
