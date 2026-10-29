use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum PstValue {
    Num(f64),
    Str(String),
    Array(Vec<PstValue>),
}

impl PstValue {
    pub fn as_f64(&self) -> f64 {
        match self {
            PstValue::Num(n) => *n,
            PstValue::Str(s) => s.parse().unwrap_or(0.0),
            PstValue::Array(_) => 0.0,
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            PstValue::Num(n) => *n != 0.0,
            PstValue::Str(s) => !s.is_empty(),
            PstValue::Array(_) => true,
        }
    }
}

impl From<i64> for PstValue {
    fn from(v: i64) -> Self {
        PstValue::Num(v as f64)
    }
}

impl From<f64> for PstValue {
    fn from(v: f64) -> Self {
        PstValue::Num(v)
    }
}

impl From<&str> for PstValue {
    fn from(v: &str) -> Self {
        PstValue::Str(v.to_string())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    String(String),
    Number(f64),
    Ident(String),
    Op(String),
    Eof,
}

impl Token {
    fn value(&self) -> &str {
        match self {
            Token::String(s) => s,
            Token::Number(n) => {
                let _ = n;
                ""
            }
            Token::Ident(s) => s,
            Token::Op(s) => s,
            Token::Eof => "<eof>",
        }
    }

    fn is_eof(&self) -> bool {
        matches!(self, Token::Eof)
    }
}

const TYPE_NAMES: [&str; 3] = ["int", "string", "float"];
const TWO_CHAR: [&str; 12] = [
    "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=",
];

pub fn tokenize(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }

        if ch == '#' && (i == 0 || chars[i - 1] == '\n') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if ch == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if ch == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if ch == '"' {
            let mut value = String::new();
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    value.push(match chars[i] {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                } else {
                    value.push(chars[i]);
                }
                i += 1;
            }
            i += 1;
            tokens.push(Token::String(value));
            continue;
        }
        if ch.is_ascii_digit()
            || (ch == '.' && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()))
        {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            tokens.push(Token::Number(text.parse().unwrap_or(0.0)));
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            tokens.push(Token::Ident(text));
            continue;
        }
        let pair: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        if TWO_CHAR.contains(&pair.as_str()) {
            tokens.push(Token::Op(pair));
            i += 2;
            continue;
        }
        tokens.push(Token::Op(ch.to_string()));
        i += 1;
    }
    tokens.push(Token::Eof);
    tokens
}

#[derive(Debug, Clone)]
pub struct Program {
    pub globals: Vec<Declaration>,
    pub functions: HashMap<String, FunctionDef>,
}

#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub name: String,
    pub return_type: Option<String>,
    pub params: Vec<String>,
    pub body: Statement,
}

#[derive(Debug, Clone)]
pub struct Declaration {
    pub value_type: String,
    pub name: String,

    pub size: Option<Expr>,
    pub init: Option<Expr>,
}

#[derive(Debug, Clone)]
pub enum Statement {
    Block(Vec<Statement>),
    Empty,
    If {
        test: Expr,
        consequent: Box<Statement>,
        alternate: Option<Box<Statement>>,
    },
    For {
        init: Option<Box<Statement>>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Box<Statement>,
    },
    While {
        test: Expr,
        body: Box<Statement>,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    Declarations(Vec<Declaration>),
    Expression(Expr),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Literal(PstValue),
    Ident(String),
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },
    Unary {
        op: String,
        value: Box<Expr>,
    },
    Binary {
        op: String,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Assign {
        op: String,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    Postfix {
        op: String,
        target: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
}

impl Parser {
    fn peek(&self, offset: usize) -> &Token {
        self.tokens
            .get(self.index + offset)
            .unwrap_or(self.tokens.last().expect("eof token"))
    }

    fn take(&mut self) -> Token {
        let token = self.peek(0).clone();
        if !token.is_eof() {
            self.index += 1;
        }
        token
    }

    fn take_value(&mut self, value: &str) -> Result<Token, String> {
        let token = self.peek(0).clone();
        if token.value() != value {
            return Err(format!("Expected {value}, got {}", token.value()));
        }
        self.index += 1;
        Ok(token)
    }

    fn match_value(&mut self, value: &str) -> bool {
        if self.peek(0).value() == value {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn program(&mut self) -> Result<Program, String> {
        let mut globals = Vec::new();
        let mut functions = HashMap::new();
        while !self.peek(0).is_eof() {
            let mut return_type = None;
            if TYPE_NAMES.contains(&self.peek(0).value()) {
                return_type = Some(self.take().value().to_string());
            }
            let name_token = self.take();
            if !matches!(name_token, Token::Ident(_)) {
                return Err(format!(
                    "Expected top-level name, got {}",
                    name_token.value()
                ));
            }
            let name = name_token.value().to_string();
            if self.peek(0).value() == "(" {
                if let Some(fn_def) = self.function_definition(&name, return_type)? {
                    functions.insert(fn_def.name.clone(), fn_def);
                }
            } else {
                if return_type.is_none() {
                    return Err(format!("Global {name} has no type"));
                }
                globals.extend(self.declaration_tail(&return_type.unwrap(), &name, true)?);
            }
        }
        Ok(Program { globals, functions })
    }

    fn function_definition(
        &mut self,
        name: &str,
        return_type: Option<String>,
    ) -> Result<Option<FunctionDef>, String> {
        self.take_value("(")?;
        let mut params = Vec::new();
        while self.peek(0).value() != ")" && !self.peek(0).is_eof() {
            if self.match_value("...") {
                continue;
            }
            if TYPE_NAMES.contains(&self.peek(0).value()) {
                self.take();
            }
            if matches!(self.peek(0), Token::Ident(_)) {
                params.push(self.take().value().to_string());
            } else {
                self.take();
            }
            if !self.match_value(",") {
                break;
            }
        }
        self.take_value(")")?;
        if self.match_value(";") {
            return Ok(None);
        }
        let body = self.block()?;
        Ok(Some(FunctionDef {
            name: name.to_string(),
            return_type,
            params,
            body,
        }))
    }

    fn declaration_tail(
        &mut self,
        value_type: &str,
        first_name: &str,
        consume_semicolon: bool,
    ) -> Result<Vec<Declaration>, String> {
        let mut declarations = Vec::new();
        let mut name = first_name.to_string();
        loop {
            let mut size = None;
            if self.match_value("[") {
                size = Some(self.expression(0)?);
                self.take_value("]")?;
            }
            let init = if self.match_value("=") {
                Some(self.expression(0)?)
            } else {
                None
            };
            declarations.push(Declaration {
                value_type: value_type.to_string(),
                name: name.clone(),
                size,
                init,
            });
            if !self.match_value(",") {
                break;
            }
            name = self.take().value().to_string();
        }
        if consume_semicolon {
            self.take_value(";")?;
        }
        Ok(declarations)
    }

    fn block(&mut self) -> Result<Statement, String> {
        self.take_value("{")?;
        let mut statements = Vec::new();
        while self.peek(0).value() != "}" && !self.peek(0).is_eof() {
            statements.push(self.statement()?);
        }
        self.take_value("}")?;
        Ok(Statement::Block(statements))
    }

    fn statement(&mut self) -> Result<Statement, String> {
        if self.peek(0).value() == "{" {
            return self.block();
        }
        if self.match_value(";") {
            return Ok(Statement::Empty);
        }
        if self.match_value("if") {
            self.take_value("(")?;
            let test = self.expression(0)?;
            self.take_value(")")?;
            let consequent = self.statement()?;
            let alternate = if self.match_value("else") {
                Some(Box::new(self.statement()?))
            } else {
                None
            };
            return Ok(Statement::If {
                test,
                consequent: Box::new(consequent),
                alternate,
            });
        }
        if self.match_value("for") {
            self.take_value("(")?;
            let init = if TYPE_NAMES.contains(&self.peek(0).value()) {
                let value_type = self.take().value().to_string();
                let name = self.take().value().to_string();
                let decls = self.declaration_tail(&value_type, &name, false)?;
                Some(Box::new(Statement::Declarations(decls)))
            } else if self.peek(0).value() != ";" {
                Some(Box::new(Statement::Expression(self.expression(0)?)))
            } else {
                None
            };
            self.take_value(";")?;
            let test = if self.peek(0).value() == ";" {
                None
            } else {
                Some(self.expression(0)?)
            };
            self.take_value(";")?;
            let update = if self.peek(0).value() == ")" {
                None
            } else {
                Some(self.expression(0)?)
            };
            self.take_value(")")?;
            let body = self.statement()?;
            return Ok(Statement::For {
                init,
                test,
                update,
                body: Box::new(body),
            });
        }
        if self.match_value("while") {
            self.take_value("(")?;
            let test = self.expression(0)?;
            self.take_value(")")?;
            let body = self.statement()?;
            return Ok(Statement::While {
                test,
                body: Box::new(body),
            });
        }
        if self.match_value("return") {
            let value = if self.peek(0).value() == ";" {
                None
            } else {
                Some(self.expression(0)?)
            };
            self.take_value(";")?;
            return Ok(Statement::Return(value));
        }
        if self.match_value("break") {
            self.take_value(";")?;
            return Ok(Statement::Break);
        }
        if self.match_value("continue") {
            self.take_value(";")?;
            return Ok(Statement::Continue);
        }
        if TYPE_NAMES.contains(&self.peek(0).value()) {
            let value_type = self.take().value().to_string();
            let name = self.take().value().to_string();
            let declarations = self.declaration_tail(&value_type, &name, true)?;
            return Ok(Statement::Declarations(declarations));
        }
        let expression = self.expression(0)?;
        self.take_value(";")?;
        Ok(Statement::Expression(expression))
    }

    fn expression(&mut self, min_precedence: u8) -> Result<Expr, String> {
        let mut left = self.unary()?;
        loop {
            let op = self.peek(0).value().to_string();
            let level = precedence(&op);
            if level <= min_precedence {
                break;
            }
            self.take();
            let right_associative = level == 1;
            let right = self.expression(if right_associative { level - 1 } else { level })?;
            left = if level == 1 {
                Expr::Assign {
                    op,
                    target: Box::new(left),
                    value: Box::new(right),
                }
            } else {
                Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                }
            };
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        let op = self.peek(0).value().to_string();
        if ["!", "-", "+"].contains(&op.as_str()) {
            self.take();
            let value = self.unary()?;
            return Ok(Expr::Unary {
                op,
                value: Box::new(value),
            });
        }
        let mut node = self.primary()?;
        loop {
            if self.match_value("(") {
                let mut args = Vec::new();
                while self.peek(0).value() != ")" {
                    args.push(self.expression(0)?);
                    if !self.match_value(",") {
                        break;
                    }
                }
                self.take_value(")")?;
                node = Expr::Call {
                    callee: Box::new(node),
                    args,
                };
            } else if self.match_value("[") {
                let index = self.expression(0)?;
                self.take_value("]")?;
                node = Expr::Index {
                    target: Box::new(node),
                    index: Box::new(index),
                };
            } else if self.match_value("++") {
                node = Expr::Postfix {
                    op: "++".into(),
                    target: Box::new(node),
                };
            } else if self.match_value("--") {
                node = Expr::Postfix {
                    op: "--".into(),
                    target: Box::new(node),
                };
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        let token = self.take();
        match token {
            Token::Number(n) => Ok(Expr::Literal(PstValue::Num(n))),
            Token::String(s) => Ok(Expr::Literal(PstValue::Str(s))),
            Token::Ident(name) => Ok(Expr::Ident(name)),
            Token::Op(op) if op == "(" => {
                let node = self.expression(0)?;
                self.take_value(")")?;
                Ok(node)
            }
            other => Err(format!("Unexpected expression token {}", other.value())),
        }
    }
}

fn precedence(op: &str) -> u8 {
    match op {
        "=" | "+=" | "-=" | "*=" | "/=" => 1,
        "||" => 2,
        "&&" => 3,
        "==" | "!=" => 4,
        "<" | "<=" | ">" | ">=" => 5,
        "+" | "-" => 6,
        "*" | "/" | "%" => 7,
        _ => 0,
    }
}

pub fn parse_program(source: &str) -> Result<Program, String> {
    Parser {
        tokens: tokenize(source),
        index: 0,
    }
    .program()
}

pub trait PstHost {
    fn builtin(&mut self, name: &str, args: &[PstValue]) -> PstValue;
    fn builtin_for_actor(
        &mut self,
        _actor: Option<i64>,
        name: &str,
        args: &[PstValue],
    ) -> PstValue {
        self.builtin(name, args)
    }

    fn update_ai_attributes(&mut self, _id: i64, _globals: &HashMap<String, PstValue>) -> bool {
        false
    }
    fn script_actor_active(&self, _id: i64) -> bool {
        true
    }
    fn initialize_script_actor(&mut self, _id: i64, _globals: &HashMap<String, PstValue>) {}
}

pub struct NullHost;

impl PstHost for NullHost {
    fn builtin(&mut self, _name: &str, _args: &[PstValue]) -> PstValue {
        PstValue::Num(0.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Flow {
    Return(PstValue),
    Break,
    Continue,
}

pub struct Runtime<H: PstHost> {
    program: Program,
    host: H,
    max_steps: usize,
    steps: usize,
    globals: HashMap<String, PstValue>,

    logs: Vec<String>,
    scripts: std::collections::BTreeMap<i64, (Program, HashMap<String, PstValue>)>,
    script_templates: std::collections::BTreeMap<i64, (Program, HashMap<String, PstValue>)>,
    initialized_scripts: std::collections::BTreeSet<i64>,
    active_script: Option<i64>,
    call_depth: usize,
}

impl<H: PstHost> Runtime<H> {
    pub fn new(program: Program, host: H, max_steps: usize) -> Self {
        let mut runtime = Self {
            program,
            host,
            max_steps,
            steps: 0,
            globals: HashMap::new(),
            logs: Vec::new(),
            scripts: Default::default(),
            script_templates: Default::default(),
            initialized_scripts: Default::default(),
            active_script: None,
            call_depth: 0,
        };
        let globals = runtime.program.globals.clone();
        for declaration in &globals {
            runtime.declare(declaration, true);
        }
        runtime
    }

    pub fn register_script(&mut self, id: i64, source: &str) -> Result<(), String> {
        let program = parse_program(source)?;
        let initial = Runtime::new(program.clone(), NullHost, self.max_steps);
        self.script_templates
            .insert(id, (program.clone(), initial.globals.clone()));
        self.scripts.insert(id, (program, initial.globals));
        Ok(())
    }

    pub fn script_initialized(&self, id: i64) -> bool {
        self.initialized_scripts.contains(&id)
    }

    pub fn activate_script(&mut self, id: i64) -> Result<(), String> {
        self.steps = 0;
        self.activate_script_inner(id)
    }

    fn activate_script_inner(&mut self, id: i64) -> Result<(), String> {
        let Some((program, globals)) = self.script_templates.get(&id).cloned() else {
            return Ok(());
        };
        if self.active_script == Some(id) {
            return Err("cannot reactivate the executing NPC script".into());
        }
        self.host.initialize_script_actor(id, &globals);
        self.scripts.insert(id, (program, globals));
        self.initialized_scripts.insert(id);
        self.call_script_inner(id, "OnInit", &[PstValue::Num(id as f64)])?;
        self.call_script_inner(id, "OnStart", &[])?;
        Ok(())
    }

    pub fn script_ids(&self) -> Vec<i64> {
        self.scripts.keys().copied().collect()
    }

    pub fn call_script(&mut self, id: i64, name: &str, args: &[PstValue]) -> Result<bool, String> {
        self.steps = 0;
        self.call_script_inner(id, name, args)
    }

    fn call_script_inner(
        &mut self,
        id: i64,
        name: &str,
        args: &[PstValue],
    ) -> Result<bool, String> {
        if !self.host.script_actor_active(id) {
            return Ok(false);
        }
        if self.active_script == Some(id) {
            if !self.has_function(name) {
                return Ok(false);
            }
            self.call_inner(name, args)?;
            return Ok(true);
        }
        let Some((program, globals)) = self.scripts.remove(&id) else {
            return Ok(false);
        };
        if !program.functions.contains_key(name) {
            self.scripts.insert(id, (program, globals));
            return Ok(false);
        }
        let previous_id = self.active_script.replace(id);
        let previous_program = std::mem::replace(&mut self.program, program);
        let previous_globals = std::mem::replace(&mut self.globals, globals);

        if let Some(caller) = previous_id {
            self.scripts
                .insert(caller, (previous_program.clone(), previous_globals.clone()));
        }
        let result = self.call_inner(name, args);
        let (restore_program, restore_globals) = previous_id
            .and_then(|caller| self.scripts.remove(&caller))
            .unwrap_or((previous_program, previous_globals));
        let program = std::mem::replace(&mut self.program, restore_program);
        let globals = std::mem::replace(&mut self.globals, restore_globals);
        self.active_script = previous_id;
        self.scripts.insert(id, (program, globals));
        result.map(|_| true)
    }

    pub fn has_function(&self, name: &str) -> bool {
        self.program.functions.contains_key(name)
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    pub fn logs(&self) -> &[String] {
        &self.logs
    }

    fn step(&mut self) -> Result<(), String> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(format!("PScript step limit {} exceeded", self.max_steps));
        }
        Ok(())
    }

    pub fn call(&mut self, name: &str, args: &[PstValue]) -> Result<PstValue, String> {
        self.steps = 0;
        self.call_inner(name, args)
    }

    fn call_inner(&mut self, name: &str, args: &[PstValue]) -> Result<PstValue, String> {
        if self.call_depth >= 64 {
            return Err("PScript call depth limit exceeded".into());
        }
        self.call_depth += 1;
        let result = self.call_inner_body(name, args);
        self.call_depth -= 1;
        result
    }

    fn call_inner_body(&mut self, name: &str, args: &[PstValue]) -> Result<PstValue, String> {
        self.step()?;
        if name == "AddNPC" {
            let result = self.host.builtin(name, args);
            let id = result.as_f64() as i64;
            if id > 0 {
                self.activate_script_inner(id)?;
            }
            return Ok(result);
        }
        if name == "RemoveNPC" {
            let result = self.host.builtin(name, args);
            if result.truthy() {
                if let Some(id) = args.first() {
                    self.initialized_scripts.remove(&(id.as_f64() as i64));
                }
            }
            return Ok(result);
        }
        if name == "Call" {
            let (Some(id), Some(PstValue::Str(function))) = (args.first(), args.get(1)) else {
                return Ok(PstValue::Num(0.0));
            };
            return self
                .call_script_inner(id.as_f64() as i64, function, &args[2..])
                .map(|called| PstValue::Num(called as u8 as f64));
        }
        if name == "UpdateAIAttrib" {
            let id = args.first().map(PstValue::as_f64).unwrap_or(-1.0) as i64;
            if !self.host.script_actor_active(id) {
                return Ok(PstValue::Num(0.0));
            }
            let globals = if self.active_script == Some(id) {
                Some(&self.globals)
            } else {
                self.scripts.get(&id).map(|(_, globals)| globals)
            };
            let updated =
                globals.is_some_and(|globals| self.host.update_ai_attributes(id, globals));
            return Ok(PstValue::Num(updated as u8 as f64));
        }
        if let Some(fn_def) = self.program.functions.get(name).cloned() {
            let mut scope = HashMap::new();
            for (index, param) in fn_def.params.iter().enumerate() {
                scope.insert(
                    param.clone(),
                    args.get(index).cloned().unwrap_or(PstValue::Num(0.0)),
                );
            }
            let result = self.execute(&fn_def.body, &mut scope)?;
            return Ok(match result {
                Some(Flow::Return(value)) => value,
                _ => PstValue::Num(0.0),
            });
        }

        let lowered = name.to_lowercase();
        let builtin = if self.host_has(name) {
            Some(name)
        } else if self.host_has(&lowered) {
            Some(lowered.as_str())
        } else {
            None
        };
        Ok(match builtin {
            Some(b) => self.host.builtin_for_actor(self.active_script, b, args),
            None => PstValue::Num(0.0),
        })
    }

    fn host_has(&self, name: &str) -> bool {
        let _ = name;
        true
    }

    fn default_value(value_type: &str) -> PstValue {
        if value_type == "string" {
            PstValue::Str(String::new())
        } else {
            PstValue::Num(0.0)
        }
    }

    fn declare(&mut self, declaration: &Declaration, global: bool) {
        let value = if let Some(size) = &declaration.size {
            let size = self
                .evaluate(size, &mut HashMap::new())
                .unwrap_or(PstValue::Num(0.0));
            let n = (size.as_f64().max(0.0)) as usize;
            PstValue::Array(vec![Self::default_value(&declaration.value_type); n])
        } else if let Some(init) = &declaration.init {
            let mut scope = HashMap::new();
            self.evaluate(init, &mut scope)
                .unwrap_or_else(|_| Self::default_value(&declaration.value_type))
        } else {
            Self::default_value(&declaration.value_type)
        };
        if global {
            self.globals.insert(declaration.name.clone(), value);
        } else {
            let _ = value;
        }
    }

    fn declare_local(
        &mut self,
        declaration: &Declaration,
        scope: &mut HashMap<String, PstValue>,
    ) -> Result<(), String> {
        if let Some(size) = &declaration.size {
            let size = self.evaluate(size, scope)?;
            let n = size.as_f64().max(0.0) as usize;
            scope.insert(
                declaration.name.clone(),
                PstValue::Array(vec![Self::default_value(&declaration.value_type); n]),
            );
        } else {
            let value = match &declaration.init {
                Some(init) => self.evaluate(init, scope)?,
                None => Self::default_value(&declaration.value_type),
            };
            scope.insert(declaration.name.clone(), value);
        }
        Ok(())
    }

    fn get(&self, name: &str, scope: &HashMap<String, PstValue>) -> PstValue {
        scope
            .get(name)
            .cloned()
            .or_else(|| self.globals.get(name).cloned())
            .unwrap_or(PstValue::Num(0.0))
    }

    fn set_reference(
        &mut self,
        target: &Expr,
        value: PstValue,
        scope: &mut HashMap<String, PstValue>,
    ) -> PstValue {
        match target {
            Expr::Ident(name) => {
                if scope.contains_key(name) {
                    scope.insert(name.clone(), value.clone());
                } else {
                    self.globals.insert(name.clone(), value.clone());
                }
                value
            }
            Expr::Index { target, index } => {
                let array = self.evaluate(target, scope).unwrap_or(PstValue::Num(0.0));
                let index = self.evaluate(index, scope).unwrap_or(PstValue::Num(0.0));
                if let PstValue::Array(mut items) = array {
                    let idx = index.as_f64() as usize;
                    if let Some(slot) = items.get_mut(idx) {
                        *slot = value.clone();

                        self.set_reference(target, PstValue::Array(items), scope);
                    }
                }
                value
            }
            _ => value,
        }
    }

    fn execute(
        &mut self,
        node: &Statement,
        scope: &mut HashMap<String, PstValue>,
    ) -> Result<Option<Flow>, String> {
        self.step()?;
        match node {
            Statement::Block(statements) => {
                for statement in statements {
                    if let Some(flow) = self.execute(statement, scope)? {
                        return Ok(Some(flow));
                    }
                }
                Ok(None)
            }
            Statement::Empty => Ok(None),
            Statement::Declarations(declarations) => {
                for declaration in declarations {
                    self.declare_local(declaration, scope)?;
                }
                Ok(None)
            }
            Statement::Expression(expression) => {
                self.evaluate(expression, scope)?;
                Ok(None)
            }
            Statement::If {
                test,
                consequent,
                alternate,
            } => {
                let test = self.evaluate(test, scope)?;
                if test.truthy() {
                    self.execute(consequent, scope)
                } else {
                    match alternate {
                        Some(alt) => self.execute(alt, scope),
                        None => Ok(None),
                    }
                }
            }
            Statement::For {
                init,
                test,
                update,
                body,
            } => {
                if let Some(init) = init {
                    self.execute(init, scope)?;
                }
                let mut guard = 0usize;
                loop {
                    if let Some(test) = test {
                        if !self.evaluate(test, scope)?.truthy() {
                            break;
                        }
                    }
                    guard += 1;
                    if guard > self.max_steps {
                        return Err("PScript for-loop limit exceeded".into());
                    }
                    if let Some(flow) = self.execute(body, scope)? {
                        match flow {
                            Flow::Return(_) => return Ok(Some(flow)),
                            Flow::Break => break,
                            Flow::Continue => {}
                        }
                    }
                    if let Some(update) = update {
                        self.evaluate(update, scope)?;
                    }
                }
                Ok(None)
            }
            Statement::While { test, body } => {
                let mut guard = 0usize;
                while self.evaluate(test, scope)?.truthy() {
                    guard += 1;
                    if guard > self.max_steps {
                        return Err("PScript while-loop limit exceeded".into());
                    }
                    if let Some(flow) = self.execute(body, scope)? {
                        match flow {
                            Flow::Return(_) => return Ok(Some(flow)),
                            Flow::Break => break,
                            Flow::Continue => {}
                        }
                    }
                }
                Ok(None)
            }
            Statement::Return(value) => {
                let value = match value {
                    Some(expr) => self.evaluate(expr, scope)?,
                    None => PstValue::Num(0.0),
                };
                Ok(Some(Flow::Return(value)))
            }
            Statement::Break => Ok(Some(Flow::Break)),
            Statement::Continue => Ok(Some(Flow::Continue)),
        }
    }

    fn evaluate(
        &mut self,
        node: &Expr,
        scope: &mut HashMap<String, PstValue>,
    ) -> Result<PstValue, String> {
        self.step()?;
        match node {
            Expr::Literal(value) => Ok(value.clone()),
            Expr::Ident(name) => Ok(self.get(name, scope)),
            Expr::Index { target, index } => {
                let target = self.evaluate(target, scope)?;
                let index = self.evaluate(index, scope)?;
                match target {
                    PstValue::Array(items) => {
                        let idx = index.as_f64() as usize;
                        Ok(items.get(idx).cloned().unwrap_or(PstValue::Num(0.0)))
                    }
                    _ => Ok(PstValue::Num(0.0)),
                }
            }
            Expr::Unary { op, value } => {
                let value = self.evaluate(value, scope)?;
                match op.as_str() {
                    "!" => Ok(PstValue::Num(if value.truthy() { 0.0 } else { 1.0 })),
                    "-" => Ok(PstValue::Num(-value.as_f64())),
                    _ => Ok(PstValue::Num(value.as_f64())),
                }
            }
            Expr::Binary { op, left, right } => {
                if op == "&&" {
                    let left = self.evaluate(left, scope)?;
                    if !left.truthy() {
                        return Ok(PstValue::Num(0.0));
                    }
                    let right = self.evaluate(right, scope)?;
                    return Ok(PstValue::Num(if right.truthy() { 1.0 } else { 0.0 }));
                }
                if op == "||" {
                    let left = self.evaluate(left, scope)?;
                    if left.truthy() {
                        return Ok(PstValue::Num(1.0));
                    }
                    let right = self.evaluate(right, scope)?;
                    return Ok(PstValue::Num(if right.truthy() { 1.0 } else { 0.0 }));
                }
                let left = self.evaluate(left, scope)?;
                let right = self.evaluate(right, scope)?;
                Ok(binary_op(op, &left, &right))
            }
            Expr::Assign { op, target, value } => {
                let current = self.evaluate(target, scope)?;
                let incoming = self.evaluate(value, scope)?;
                let value = match op.as_str() {
                    "=" => incoming.clone(),
                    "+=" => num_op(&current, &incoming, "+"),
                    "-=" => num_op(&current, &incoming, "-"),
                    "*=" => num_op(&current, &incoming, "*"),
                    "/=" => {
                        let r = incoming.as_f64();
                        if r == 0.0 {
                            PstValue::Num(0.0)
                        } else {
                            PstValue::Num((current.as_f64() / r).trunc())
                        }
                    }
                    _ => PstValue::Num(0.0),
                };
                Ok(self.set_reference(target, value, scope))
            }
            Expr::Postfix { op, target } => {
                let before = self.evaluate(target, scope)?.as_f64();
                let delta = if op == "++" { 1.0 } else { -1.0 };
                self.set_reference(target, PstValue::Num(before + delta), scope);
                Ok(PstValue::Num(before))
            }
            Expr::Call { callee, args } => {
                let name = match callee.as_ref() {
                    Expr::Ident(name) => name.clone(),
                    _ => String::new(),
                };

                if name == "MakeString" && !args.is_empty() {
                    let value = args[1..]
                        .iter()
                        .map(|arg| {
                            let v = self.evaluate(arg, scope)?;
                            Ok(match v {
                                PstValue::Num(n) if n.fract().abs() < 1e-9 => {
                                    format!("{:02}", n.trunc() as i64)
                                }
                                other => other_to_string(&other),
                            })
                        })
                        .collect::<Result<Vec<String>, String>>()?
                        .join("");
                    self.set_reference(&args[0], PstValue::Str(value), scope);
                    return Ok(PstValue::Num(1.0));
                }
                let mut evaluated = Vec::with_capacity(args.len());
                for arg in args {
                    evaluated.push(self.evaluate(arg, scope)?);
                }
                if self.program.functions.contains_key(&name) {
                    return self.call_inner(&name, &evaluated);
                }

                self.call_inner(&name, &evaluated)
            }
        }
    }
}

fn num_op(left: &PstValue, right: &PstValue, op: &str) -> PstValue {
    match op {
        "+" => match (left, right) {
            (PstValue::Str(a), b) => PstValue::Str(format!("{a}{}", other_to_string(b))),
            (a, PstValue::Str(b)) => PstValue::Str(format!("{}{b}", other_to_string(a))),
            _ => PstValue::Num(left.as_f64() + right.as_f64()),
        },
        "-" => PstValue::Num(left.as_f64() - right.as_f64()),
        "*" => PstValue::Num(left.as_f64() * right.as_f64()),
        _ => PstValue::Num(0.0),
    }
}

fn binary_op(op: &str, left: &PstValue, right: &PstValue) -> PstValue {
    match op {
        "==" => PstValue::Num(if left == right { 1.0 } else { 0.0 }),
        "!=" => PstValue::Num(if left != right { 1.0 } else { 0.0 }),
        "<" => PstValue::Num(if left.as_f64() < right.as_f64() {
            1.0
        } else {
            0.0
        }),
        "<=" => PstValue::Num(if left.as_f64() <= right.as_f64() {
            1.0
        } else {
            0.0
        }),
        ">" => PstValue::Num(if left.as_f64() > right.as_f64() {
            1.0
        } else {
            0.0
        }),
        ">=" => PstValue::Num(if left.as_f64() >= right.as_f64() {
            1.0
        } else {
            0.0
        }),
        "+" => num_op(left, right, "+"),
        "-" => PstValue::Num(left.as_f64() - right.as_f64()),
        "*" => PstValue::Num(left.as_f64() * right.as_f64()),

        "/" => {
            let r = right.as_f64();
            if r == 0.0 {
                PstValue::Num(0.0)
            } else {
                PstValue::Num((left.as_f64() / r).trunc())
            }
        }
        "%" => {
            let r = right.as_f64();
            if r == 0.0 {
                PstValue::Num(0.0)
            } else {
                PstValue::Num(left.as_f64() % r)
            }
        }
        _ => PstValue::Num(0.0),
    }
}

fn other_to_string(value: &PstValue) -> String {
    match value {
        PstValue::Num(n) => {
            if n.fract().abs() < 1e-9 {
                format!("{}", n.trunc() as i64)
            } else {
                format!("{n}")
            }
        }
        PstValue::Str(s) => s.clone(),
        PstValue::Array(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct LoggingHost {
        calls: Vec<String>,
    }

    impl PstHost for LoggingHost {
        fn builtin(&mut self, name: &str, args: &[PstValue]) -> PstValue {
            self.calls.push(name.to_string());
            let _ = args;
            PstValue::Num(7.0)
        }
    }

    #[test]
    fn tokenizes_basic_program() {
        let tokens = tokenize(
            r#"int g = 5; // comment
/* block */
f() { if (g >= 3) { g += 1; } }"#,
        );
        assert!(matches!(tokens[0], Token::Ident(ref s) if s == "int"));
        assert!(tokens
            .iter()
            .any(|t| matches!(t, Token::Op(ref s) if s == ">=")));
        assert!(!tokens.iter().any(|t| t.value().contains("comment")));
    }

    #[test]
    fn parses_globals_and_function() {
        let source = "int g_iStoneMaxHP = 500;\nOnLoad() { g_iStoneMaxHP = g_iStoneMaxHP + 10; }";
        let program = parse_program(source).expect("parse");
        assert_eq!(program.globals.len(), 1);
        assert_eq!(program.globals[0].name, "g_iStoneMaxHP");
        assert!(program.functions.contains_key("OnLoad"));
    }

    #[test]
    fn for_loop_sums() {
        let source = r#"
int total = 0;
OnLoad() {
  int i;
  for (i = 0; i < 10; i++) { total = total + i; }
}"#;
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, NullHost, 20_000);
        runtime.call("OnLoad", &[]).expect("call");
        assert_eq!(runtime.globals.get("total").unwrap().as_f64(), 45.0);
    }

    #[test]
    fn indexed_assignments_persist_to_global_and_local_arrays() {
        let source = r#"
int alive[3];
int global_value;
int local_value;
OnLoad() {
  int local[2];
  alive[1] = 1;
  alive[1] += 2;
  local[0] = 7;
  local[0]++;
  global_value = alive[1];
  local_value = local[0];
}"#;
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, NullHost, 20_000);
        runtime.call("OnLoad", &[]).expect("call");
        assert_eq!(runtime.globals.get("global_value").unwrap().as_f64(), 3.0);
        assert_eq!(runtime.globals.get("local_value").unwrap().as_f64(), 8.0);
    }

    #[test]
    fn while_loop_with_break() {
        let source = r#"
int n = 0;
OnLoad() {
  while (1) {
    n = n + 1;
    if (n >= 5) { break; }
  }
}"#;
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, NullHost, 20_000);
        runtime.call("OnLoad", &[]).expect("call");
        assert_eq!(runtime.globals.get("n").unwrap().as_f64(), 5.0);
    }

    #[test]
    fn integer_division_and_modulo() {
        let source = r#"
int a; int b;
OnLoad() {
  a = 7 / 2;
  b = 7 % 3;
}"#;
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, NullHost, 20_000);
        runtime.call("OnLoad", &[]).expect("call");
        assert_eq!(runtime.globals.get("a").unwrap().as_f64(), 3.0);
        assert_eq!(runtime.globals.get("b").unwrap().as_f64(), 1.0);
    }

    #[test]
    fn string_concat_with_plus() {
        let source = r#"
string s;
OnLoad() {
  s = "D_BBox" + "01";
}"#;
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, NullHost, 20_000);
        runtime.call("OnLoad", &[]).expect("call");
        assert_eq!(
            runtime.globals.get("s").unwrap(),
            &PstValue::Str("D_BBox01".into())
        );
    }

    #[test]
    fn make_string_pads_numbers() {
        let source = r#"
string s;
OnLoad() {
  MakeString(s, "D_BBox", 1);
}"#;
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, NullHost, 20_000);
        runtime.call("OnLoad", &[]).expect("call");
        assert_eq!(
            runtime.globals.get("s").unwrap(),
            &PstValue::Str("D_BBox01".into())
        );
    }

    #[test]
    fn make_string_joins_parts() {
        let source = r#"
string s;
OnLoad() {
  MakeString(s, "D_it", "RJug", 10, "01");
}"#;
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, NullHost, 20_000);
        runtime.call("OnLoad", &[]).expect("call");
        assert_eq!(
            runtime.globals.get("s").unwrap(),
            &PstValue::Str("D_itRJug1001".into())
        );
    }

    #[test]
    fn builtin_call_returns_host_value() {
        let source = "int x; OnLoad() { x = Foo(1, 2); }";
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, LoggingHost { calls: Vec::new() }, 20_000);
        runtime.call("OnLoad", &[]).expect("call");
        assert_eq!(runtime.globals.get("x").unwrap().as_f64(), 7.0);
    }

    #[test]
    fn unknown_function_returns_zero() {
        let source = "int x; OnLoad() { x = NoSuchFn(1); }";
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, NullHost, 20_000);
        runtime.call("OnLoad", &[]).expect("call");
        assert_eq!(runtime.globals.get("x").unwrap().as_f64(), 0.0);
    }

    #[test]
    fn step_limit_guard() {
        let source = "OnLoad() { while (1) { } }";
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, NullHost, 200);
        let result = runtime.call("OnLoad", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn return_value_propagates() {
        let source = "int add(int a, int b) { return a + b; } int x; OnLoad() { x = add(3, 4); }";
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, NullHost, 20_000);
        runtime.call("OnLoad", &[]).expect("call");
        assert_eq!(runtime.globals.get("x").unwrap().as_f64(), 7.0);
    }

    #[test]
    fn string_comparison_equality() {
        let source = r#"
int r;
OnLoad() {
  if ("abc" == "abc") { r = 1; } else { r = 2; }
}"#;
        let program = parse_program(source).expect("parse");
        let mut runtime = Runtime::new(program, NullHost, 20_000);
        runtime.call("OnLoad", &[]).expect("call");
        assert_eq!(runtime.globals.get("r").unwrap().as_f64(), 1.0);
    }
}
