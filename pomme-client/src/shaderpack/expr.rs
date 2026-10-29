//! OptiFine's expression language, as `shaders.properties` custom uniforms
//! and `#if` lines use it: numbers, booleans and float vectors, the usual
//! operators, `.x`-style component access and OptiFine's functions.

use glam::{IVec2, Mat4, Vec2, Vec3, Vec4};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i32),
    Float(f32),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
    IVec2(IVec2),
    Mat4(Mat4),
}

impl Value {
    /// The value's components as floats, and how many there are; `None` for
    /// a matrix.
    pub fn components(self) -> Option<(Vec4, usize)> {
        Some(match self {
            Self::Bool(b) => (Vec4::splat(f32::from(u8::from(b))), 1),
            Self::Int(i) => (Vec4::splat(i as f32), 1),
            Self::Float(f) => (Vec4::splat(f), 1),
            Self::Vec2(v) => (v.extend(0.0).extend(0.0), 2),
            Self::Vec3(v) => (v.extend(0.0), 3),
            Self::Vec4(v) => (v, 4),
            Self::IVec2(v) => (v.as_vec2().extend(0.0).extend(0.0), 2),
            Self::Mat4(_) => return None,
        })
    }

    fn from_components(v: Vec4, len: usize) -> Self {
        match len {
            1 => Self::Float(v.x),
            2 => Self::Vec2(v.truncate().truncate()),
            3 => Self::Vec3(v.truncate()),
            _ => Self::Vec4(v),
        }
    }

    pub fn as_f32(self) -> f32 {
        self.components().map_or(0.0, |(v, _)| v.x)
    }

    pub fn as_i32(self) -> i32 {
        match self {
            Self::Int(i) => i,
            other => other.as_f32() as i32,
        }
    }

    pub fn as_bool(self) -> bool {
        match self {
            Self::Bool(b) => b,
            other => other.as_f32() != 0.0,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("{0}")]
pub struct ExprError(String);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Literal(Value),
    Variable(String),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// A function call; `smooth` calls carry their state slot.
    Call(String, Vec<Expr>, usize),
    Component(Box<Expr>, usize),
}

/// What an expression reads while evaluating.
pub struct Context<'a> {
    pub lookup: &'a dyn Fn(&str) -> Option<Value>,
    /// Seconds since the last evaluation, for `smooth`.
    pub frame_time: f32,
    /// Each `smooth` call's value, by slot; `None` before its first frame.
    pub smooth: &'a mut [Option<f32>],
    /// Names `lookup` didn't know, which read as zero.
    pub missing: Vec<String>,
}

impl Expr {
    /// Parses `text`, numbering its `smooth` calls from `smooth_slots`.
    pub fn parse(text: &str, smooth_slots: &mut usize) -> Result<Self, ExprError> {
        let tokens = lex(text)?;
        let mut parser = Parser {
            tokens,
            at: 0,
            smooth_slots,
        };
        let expr = parser.or()?;
        match parser.tokens.get(parser.at) {
            None => Ok(expr),
            Some(t) => Err(ExprError(format!("unexpected {t:?} in `{text}`"))),
        }
    }

    pub fn eval(&self, cx: &mut Context) -> Value {
        match self {
            Self::Literal(v) => *v,
            Self::Variable(name) => (cx.lookup)(name).unwrap_or_else(|| {
                cx.missing.push(name.clone());
                Value::Int(0)
            }),
            Self::Neg(e) => match e.eval(cx) {
                Value::Int(i) => Value::Int(i.wrapping_neg()),
                v => map(v, |c| -c),
            },
            Self::Not(e) => Value::Bool(!e.eval(cx).as_bool()),
            Self::Binary(op, a, b) => binary(*op, a, b, cx),
            Self::Call(name, args, slot) => call(name, args, *slot, cx),
            Self::Component(e, i) => match e.eval(cx) {
                Value::IVec2(v) => Value::Int(v.to_array().get(*i).copied().unwrap_or(0)),
                v => Value::Float(v.components().map_or(0.0, |(c, _)| c[*i])),
            },
        }
    }
}

/// `f` over each component, keeping the shape.
fn map(v: Value, f: impl Fn(f32) -> f32) -> Value {
    match v.components() {
        Some((c, len)) => Value::from_components(c.to_array().map(f).into(), len),
        None => v,
    }
}

fn binary(op: BinOp, a: &Expr, b: &Expr, cx: &mut Context) -> Value {
    // `&&` and `||` short-circuit.
    match op {
        BinOp::And => return Value::Bool(a.eval(cx).as_bool() && b.eval(cx).as_bool()),
        BinOp::Or => return Value::Bool(a.eval(cx).as_bool() || b.eval(cx).as_bool()),
        _ => {}
    }
    let (a, b) = (a.eval(cx), b.eval(cx));
    if let (Value::Int(x), Value::Int(y)) = (a, b) {
        return match op {
            BinOp::Add => Value::Int(x.wrapping_add(y)),
            BinOp::Sub => Value::Int(x.wrapping_sub(y)),
            BinOp::Mul => Value::Int(x.wrapping_mul(y)),
            BinOp::Div => Value::Int(x.checked_div(y).unwrap_or(0)),
            BinOp::Rem => Value::Int(x.checked_rem(y).unwrap_or(0)),
            _ => compare(op, x as f32, y as f32),
        };
    }
    let f: fn(f32, f32) -> f32 = match op {
        BinOp::Add => |x, y| x + y,
        BinOp::Sub => |x, y| x - y,
        BinOp::Mul => |x, y| x * y,
        BinOp::Div => |x, y| x / y,
        BinOp::Rem => |x, y| x % y,
        _ => return compare(op, a.as_f32(), b.as_f32()),
    };
    componentwise(&[a, b], |c| f(c[0], c[1]))
}

/// `f` over each component of `args`, a scalar spread to every component;
/// zero with a matrix among them.
fn componentwise(args: &[Value], f: impl Fn(&[f32]) -> f32) -> Value {
    let Some(parts) = args
        .iter()
        .map(|v| v.components())
        .collect::<Option<Vec<_>>>()
    else {
        return Value::Float(0.0);
    };
    let len = parts.iter().map(|&(_, n)| n).max().unwrap_or(1);
    let out: [f32; 4] = std::array::from_fn(|i| {
        let column: Vec<f32> = parts
            .iter()
            .map(|&(v, n)| if n == 1 { v.x } else { v[i] })
            .collect();
        f(&column)
    });
    Value::from_components(out.into(), len)
}

fn compare(op: BinOp, x: f32, y: f32) -> Value {
    Value::Bool(match op {
        BinOp::Eq => x == y,
        BinOp::Ne => x != y,
        BinOp::Lt => x < y,
        BinOp::Le => x <= y,
        BinOp::Gt => x > y,
        _ => x >= y,
    })
}

fn call(name: &str, args: &[Expr], slot: usize, cx: &mut Context) -> Value {
    // `if` evaluates only the branch it takes.
    if name == "if" {
        let (pairs, otherwise) = args.as_chunks::<2>();
        for [condition, value] in pairs {
            if condition.eval(cx).as_bool() {
                return value.eval(cx);
            }
        }
        return otherwise.first().map_or(Value::Float(0.0), |e| e.eval(cx));
    }
    let values: Vec<Value> = args.iter().map(|a| a.eval(cx)).collect();
    let f = |i: usize| values.get(i).map_or(0.0, |v| v.as_f32());
    match name {
        "sin" => map(values[0], f32::sin),
        "cos" => map(values[0], f32::cos),
        "tan" => map(values[0], f32::tan),
        "asin" => map(values[0], f32::asin),
        "acos" => map(values[0], f32::acos),
        "atan2" | "atan" if values.len() == 2 => Value::Float(f(0).atan2(f(1))),
        "atan" => map(values[0], f32::atan),
        "torad" => map(values[0], f32::to_radians),
        "todeg" => map(values[0], f32::to_degrees),
        "abs" => match values[0] {
            Value::Int(i) => Value::Int(i.wrapping_abs()),
            v => map(v, f32::abs),
        },
        "floor" => map(values[0], f32::floor),
        "ceil" => map(values[0], f32::ceil),
        "round" => map(values[0], f32::round),
        "frac" | "fract" => map(values[0], |x| x - x.floor()),
        "exp" => map(values[0], f32::exp),
        "exp2" => map(values[0], f32::exp2),
        "log" => map(values[0], f32::ln),
        "log2" => map(values[0], f32::log2),
        "sqrt" => map(values[0], f32::sqrt),
        "signum" | "sign" => map(values[0], |x| if x == 0.0 { 0.0 } else { x.signum() }),
        "pow" => Value::Float(f(0).powf(f(1))),
        "fmod" => Value::Float(f(0) - f(1) * (f(0) / f(1)).floor()),
        "min" | "max" => {
            let pick = |a: f32, b: f32| if name == "min" { a.min(b) } else { a.max(b) };
            if values.iter().all(|v| matches!(v, Value::Int(_))) {
                let ints = values.iter().map(|v| v.as_i32());
                let out = if name == "min" {
                    ints.min()
                } else {
                    ints.max()
                };
                Value::Int(out.unwrap_or(0))
            } else {
                componentwise(&values, |c| c.iter().copied().reduce(pick).unwrap_or(0.0))
            }
        }
        "clamp" => componentwise(&values, |c| c[1].max(c[2].min(c[0]))),
        "smoothstep" => {
            let t = ((f(2) - f(0)) / (f(1) - f(0))).clamp(0.0, 1.0);
            Value::Float(t * t * (3.0 - 2.0 * t))
        }
        "between" => Value::Bool(f(0) >= f(1) && f(0) <= f(2)),
        // Without an epsilon, an exact match.
        "equals" => Value::Bool((f(0) - f(1)).abs() <= values.get(2).map_or(0.0, |v| v.as_f32())),
        "in" => Value::Bool(values[1..].iter().any(|v| v.as_f32() == f(0))),
        "random" if values.len() == 2 => Value::Float(f(0) + fastrand::f32() * (f(1) - f(0))),
        "random" => Value::Float(fastrand::f32()),
        "vec2" | "vec3" | "vec4" => {
            let len = usize::from(name.as_bytes()[3] - b'0');
            let flat: Vec<f32> = values
                .iter()
                .filter_map(|v| v.components())
                .flat_map(|(c, n)| c.to_array().into_iter().take(n))
                .collect();
            let out: [f32; 4] =
                std::array::from_fn(|i| flat.get(i).or(flat.last()).copied().unwrap_or(0.0));
            Value::from_components(out.into(), len)
        }
        "smooth" => Value::Float(smooth(&values, slot, cx)),
        _ => Value::Float(0.0),
    }
}

/// `smooth(value, [fade | fadeUp, fadeDown])`, its id already dropped:
/// eases toward `value`, halving the gap every tenth of the fade time, in
/// seconds, as Iris's `SmoothFloat` does.
fn smooth(values: &[Value], slot: usize, cx: &mut Context) -> f32 {
    let args: Vec<f32> = values.iter().map(|v| v.as_f32()).collect();
    let (target, up, down) = match args[..] {
        [value, up, down] => (value, up, down),
        [value, fade] => (value, fade, fade),
        [value] => (value, 1.0, 1.0),
        _ => (0.0, 0.0, 0.0),
    };
    let Some(state) = cx.smooth.get_mut(slot) else {
        return target;
    };
    let current = state.unwrap_or(target);
    let fade = if target > current { up } else { down };
    let next = if fade <= 0.0 {
        target
    } else {
        target + (current - target) * (-cx.frame_time * 2f32.ln() / (0.1 * fade)).exp()
    };
    *state = Some(next);
    next
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(Value),
    Ident(String),
    Op(&'static str),
}

const OPERATORS: &[&str] = &[
    "&&", "||", "==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "%", "!", "(", ")", ",", ".",
];

fn lex(text: &str) -> Result<Vec<Token>, ExprError> {
    let mut tokens = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let c = rest.chars().next().expect("not empty");
        let len = if c.is_ascii_digit()
            || (c == '.' && rest[1..].starts_with(|d: char| d.is_ascii_digit()))
        {
            let len = rest
                .find(|d: char| !(d.is_ascii_alphanumeric() || d == '.'))
                .unwrap_or(rest.len());
            let number = rest[..len].trim_end_matches(['f', 'F']);
            tokens.push(Token::Number(if number.contains('.') {
                Value::Float(number.parse().map_err(|_| bad_number(number))?)
            } else {
                Value::Int(number.parse().map_err(|_| bad_number(number))?)
            }));
            len
        } else if c.is_alphabetic() || c == '_' {
            let len = rest
                .find(|d: char| !(d.is_alphanumeric() || d == '_'))
                .unwrap_or(rest.len());
            tokens.push(Token::Ident(rest[..len].to_owned()));
            len
        } else if let Some(op) = OPERATORS.iter().find(|op| rest.starts_with(**op)) {
            tokens.push(Token::Op(op));
            op.len()
        } else {
            return Err(ExprError(format!("unexpected `{c}` in `{text}`")));
        };
        rest = rest[len..].trim_start();
    }
    Ok(tokens)
}

fn bad_number(text: &str) -> ExprError {
    ExprError(format!("bad number `{text}`"))
}

struct Parser<'a> {
    tokens: Vec<Token>,
    at: usize,
    smooth_slots: &'a mut usize,
}

impl Parser<'_> {
    fn eat(&mut self, op: &str) -> bool {
        let found = matches!(self.tokens.get(self.at), Some(Token::Op(o)) if *o == op);
        if found {
            self.at += 1;
        }
        found
    }

    fn expect(&mut self, op: &str) -> Result<(), ExprError> {
        if self.eat(op) {
            Ok(())
        } else {
            Err(ExprError(format!(
                "expected `{op}`, found {:?}",
                self.tokens.get(self.at)
            )))
        }
    }

    /// One left-associative precedence level over `next`.
    fn level(
        &mut self,
        ops: &[(&str, BinOp)],
        next: fn(&mut Self) -> Result<Expr, ExprError>,
    ) -> Result<Expr, ExprError> {
        let mut left = next(self)?;
        'outer: loop {
            for (token, op) in ops {
                if self.eat(token) {
                    left = Expr::Binary(*op, Box::new(left), Box::new(next(self)?));
                    continue 'outer;
                }
            }
            return Ok(left);
        }
    }

    fn or(&mut self) -> Result<Expr, ExprError> {
        self.level(&[("||", BinOp::Or)], Self::and)
    }

    fn and(&mut self) -> Result<Expr, ExprError> {
        self.level(&[("&&", BinOp::And)], Self::equality)
    }

    fn equality(&mut self) -> Result<Expr, ExprError> {
        self.level(&[("==", BinOp::Eq), ("!=", BinOp::Ne)], Self::relation)
    }

    fn relation(&mut self) -> Result<Expr, ExprError> {
        self.level(
            &[
                ("<=", BinOp::Le),
                (">=", BinOp::Ge),
                ("<", BinOp::Lt),
                (">", BinOp::Gt),
            ],
            Self::sum,
        )
    }

    fn sum(&mut self) -> Result<Expr, ExprError> {
        self.level(&[("+", BinOp::Add), ("-", BinOp::Sub)], Self::product)
    }

    fn product(&mut self) -> Result<Expr, ExprError> {
        self.level(
            &[("*", BinOp::Mul), ("/", BinOp::Div), ("%", BinOp::Rem)],
            Self::unary,
        )
    }

    fn unary(&mut self) -> Result<Expr, ExprError> {
        if self.eat("-") {
            Ok(Expr::Neg(Box::new(self.unary()?)))
        } else if self.eat("!") {
            Ok(Expr::Not(Box::new(self.unary()?)))
        } else if self.eat("+") {
            self.unary()
        } else {
            self.postfix()
        }
    }

    fn postfix(&mut self) -> Result<Expr, ExprError> {
        let mut expr = self.primary()?;
        while self.eat(".") {
            let Some(Token::Ident(member)) = self.tokens.get(self.at).cloned() else {
                return Err(ExprError("expected a component after `.`".into()));
            };
            self.at += 1;
            let index = match member.as_str() {
                "x" | "r" | "s" => 0,
                "y" | "g" | "t" => 1,
                "z" | "b" | "p" => 2,
                "w" | "a" | "q" => 3,
                _ => return Err(ExprError(format!("unknown component `.{member}`"))),
            };
            expr = Expr::Component(Box::new(expr), index);
        }
        Ok(expr)
    }

    fn primary(&mut self) -> Result<Expr, ExprError> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        match token {
            Some(Token::Number(v)) => Ok(Expr::Literal(v)),
            Some(Token::Ident(name)) if name == "true" => Ok(Expr::Literal(Value::Bool(true))),
            Some(Token::Ident(name)) if name == "false" => Ok(Expr::Literal(Value::Bool(false))),
            Some(Token::Ident(name)) => {
                if !self.eat("(") {
                    return Ok(Expr::Variable(name));
                }
                let mut args = Vec::new();
                if !self.eat(")") {
                    loop {
                        args.push(self.or()?);
                        if self.eat(")") {
                            break;
                        }
                        self.expect(",")?;
                    }
                }
                let slot = if name == "smooth" {
                    // Iris takes a leading number as the call's id and
                    // ignores it.
                    if args.len() > 1
                        && matches!(args[0], Expr::Literal(Value::Int(_) | Value::Float(_)))
                    {
                        args.remove(0);
                    }
                    *self.smooth_slots += 1;
                    *self.smooth_slots - 1
                } else {
                    0
                };
                check_arity(&name, args.len())?;
                Ok(Expr::Call(name, args, slot))
            }
            Some(Token::Op("(")) => {
                let expr = self.or()?;
                self.expect(")")?;
                Ok(expr)
            }
            other => Err(ExprError(format!("unexpected {other:?}"))),
        }
    }
}

/// Every function [`call`] knows, with its argument counts.
fn check_arity(name: &str, count: usize) -> Result<(), ExprError> {
    let ok = match name {
        "sin" | "cos" | "tan" | "asin" | "acos" | "torad" | "todeg" | "abs" | "floor" | "ceil"
        | "round" | "frac" | "fract" | "exp" | "exp2" | "log" | "log2" | "sqrt" | "signum"
        | "sign" => count == 1,
        "atan" => count == 1 || count == 2,
        "atan2" | "pow" | "fmod" => count == 2,
        "clamp" | "smoothstep" | "between" => count == 3,
        "equals" => count == 2 || count == 3,
        "min" | "max" | "in" => count >= 2,
        "if" => count >= 3 && count % 2 == 1,
        "random" => count == 0 || count == 2,
        "smooth" => (1..=3).contains(&count),
        "vec2" | "vec3" | "vec4" => count >= 1,
        _ => return Err(ExprError(format!("unknown function `{name}`"))),
    };
    if ok {
        Ok(())
    } else {
        Err(ExprError(format!("`{name}` with {count} arguments")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_with(text: &str, vars: &[(&str, Value)]) -> Value {
        let mut slots = 0;
        let expr = Expr::parse(text, &mut slots).unwrap();
        let lookup = |name: &str| vars.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
        let mut smooth = vec![None; slots];
        expr.eval(&mut Context {
            lookup: &lookup,
            frame_time: 0.05,
            smooth: &mut smooth,
            missing: Vec::new(),
        })
    }

    fn eval(text: &str) -> Value {
        eval_with(text, &[])
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(eval("1 + 2 * 3"), Value::Int(7));
        assert_eq!(eval("(1 + 2) * 3"), Value::Int(9));
        assert_eq!(eval("7 / 2"), Value::Int(3));
        assert_eq!(eval("7 / 2.0"), Value::Float(3.5));
        assert_eq!(eval("-7 % 3"), Value::Int(-1));
        assert_eq!(eval("2 - -1"), Value::Int(3));
        assert_eq!(eval("1 < 2 && !(3 == 4) || 0"), Value::Bool(true));
    }

    #[test]
    fn functions() {
        assert_eq!(eval("if(1 > 2, 10, 0 > 1, 20, 30)"), Value::Int(30));
        assert_eq!(eval("if(true, 1.5, 2)"), Value::Float(1.5));
        assert_eq!(eval("in(3, 1, 2, 3)"), Value::Bool(true));
        assert_eq!(eval("frac(-0.25)"), Value::Float(0.75));
        assert_eq!(eval("equals(0.5, 0.5)"), Value::Bool(true));
        assert_eq!(eval("between(5, 1, 4)"), Value::Bool(false));
        assert_eq!(eval("equals(1.0, 1.05, 0.1)"), Value::Bool(true));
        assert_eq!(eval("clamp(5.0, 0.0, 1.0)"), Value::Float(1.0));
        assert_eq!(
            eval("clamp(vec3(-1.0, 0.5, 2.0), 0.0, 1.0)"),
            Value::Vec3(Vec3::new(0.0, 0.5, 1.0))
        );
        assert_eq!(eval("max(1, 4, 2)"), Value::Int(4));
        assert_eq!(eval("min(1.5, 4)"), Value::Float(1.5));
        assert_eq!(
            eval("min(vec2(1.0, 4.0), vec2(3.0, 2.0))"),
            Value::Vec2(Vec2::new(1.0, 2.0))
        );
        assert_eq!(eval("fmod(-1.0, 4.0)"), Value::Float(3.0));
        assert_eq!(eval("vec3(1.0, 2.0, 3.0).y"), Value::Float(2.0));
        assert_eq!(eval("vec2(1.0) * 2.0"), Value::Vec2(Vec2::splat(2.0)));
        assert!(Expr::parse("nope(1)", &mut 0).is_err());
        assert!(Expr::parse("clamp(1, 2)", &mut 0).is_err());
    }

    #[test]
    fn variables_and_components() {
        let vars = [
            ("eyeBrightness", Value::IVec2(IVec2::new(0, 240))),
            ("cameraPosition", Value::Vec3(Vec3::new(1.0, 2.0, 3.0))),
            ("frameCounter", Value::Int(7)),
        ];
        assert_eq!(
            eval_with("eyeBrightness.y / 240.0", &vars),
            Value::Float(1.0)
        );
        assert_eq!(eval_with("cameraPosition.z - 1", &vars), Value::Float(2.0));
        assert_eq!(eval_with("frameCounter % 4", &vars), Value::Int(3));
    }

    #[test]
    fn smooth_eases_per_call() {
        // The first frame takes the value, then the gap halves every tenth
        // of the fade time; a leading number is the id.
        for (text, eased) in [
            ("smooth(1, target, 1, 1)", 0.5),
            ("smooth(5, target, 2)", 1.0 - 0.5f32.sqrt()),
        ] {
            let mut slots = 0;
            let expr = Expr::parse(text, &mut slots).unwrap();
            assert_eq!(slots, 1);
            let mut state = vec![None; slots];
            let mut run = |target: f32, frame_time: f32| {
                let lookup = |_: &str| Some(Value::Float(target));
                expr.eval(&mut Context {
                    lookup: &lookup,
                    frame_time,
                    smooth: &mut state,
                    missing: Vec::new(),
                })
                .as_f32()
            };
            assert_eq!(run(0.0, 0.0), 0.0);
            assert!((run(1.0, 0.1) - eased).abs() < 1e-4, "{text}");
        }
    }

    #[test]
    fn complementary_cave_factor() {
        let vars = [
            ("isEyeInWater", Value::Int(0)),
            ("eyeAltitude", Value::Float(64.0)),
            ("eyeBrightness", Value::IVec2(IVec2::new(0, 240))),
        ];
        let value = eval_with(
            "if(isEyeInWater == 0, 1.0 - smooth(202, if(eyeAltitude < 5.0, eyeBrightness.y / 240.0, 1.0), 6, 12), 0.0)",
            &vars,
        );
        assert_eq!(value, Value::Float(0.0));
    }
}
