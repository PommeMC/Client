//! Iris's alpha test: the `discard` it ends a gbuffers program's fragment
//! `main` with (`AlphaTest.toExpression`), by the vanilla render type the
//! program draws or the pack's `alphaTest.<program>`.

/// `AlphaTestFunction`: GL's comparisons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Function {
    Never,
    Less,
    Equal,
    LEqual,
    Greater,
    NotEqual,
    GEqual,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlphaTest {
    function: Function,
    reference: f32,
}

impl AlphaTest {
    pub const OFF: Self = Self {
        function: Function::Always,
        reference: 0.0,
    };
    pub const NON_ZERO: Self = Self::greater(0.0001);
    pub const ONE_TENTH: Self = Self::greater(0.1);

    const fn greater(reference: f32) -> Self {
        Self {
            function: Function::Greater,
            reference,
        }
    }

    /// An `alphaTest.<program>` value: `off`, `false`, or a function and a
    /// reference, e.g. `GREATER 0.1`.
    pub fn parse(value: &str) -> Option<Self> {
        if value == "off" || value == "false" {
            return Some(Self::OFF);
        }
        let mut parts = value.split(' ');
        let function = match parts.next()? {
            "NEVER" => Function::Never,
            "LESS" => Function::Less,
            "EQUAL" => Function::Equal,
            "LEQUAL" => Function::LEqual,
            "GREATER" => Function::Greater,
            "NOTEQUAL" => Function::NotEqual,
            "GEQUAL" => Function::GEqual,
            "ALWAYS" | "GL_ALWAYS" => Function::Always,
            _ => return None,
        };
        // Java's `parseFloat`, which takes a type suffix.
        let reference = parts.next()?.trim().trim_end_matches(['f', 'F', 'd', 'D']);
        Some(Self {
            function,
            reference: reference.parse().ok()?,
        })
    }

    /// The GLSL that discards a fragment whose `alpha` fails the test.
    pub fn discard(self, alpha: &str) -> String {
        let op = match self.function {
            Function::Always => return String::new(),
            Function::Never => return "    discard;\n".to_owned(),
            Function::Less => "<",
            Function::Equal => "==",
            Function::LEqual => "<=",
            Function::Greater => ">",
            Function::NotEqual => "!=",
            Function::GEqual => ">=",
        };
        format!("    if (!({alpha} {op} {:?})) discard;\n", self.reference)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_parse_as_iris_reads_them() {
        assert_eq!(AlphaTest::parse("off"), Some(AlphaTest::OFF));
        assert_eq!(AlphaTest::parse("GL_ALWAYS 0"), Some(AlphaTest::OFF));
        assert_eq!(AlphaTest::parse("GREATER 0.1f"), Some(AlphaTest::ONE_TENTH));
        assert_eq!(AlphaTest::parse("GL_GREATER 0.1"), None);
        assert_eq!(AlphaTest::parse("GREATER"), None);
    }

    #[test]
    fn the_discard_follows_the_function() {
        assert_eq!(AlphaTest::OFF.discard("a"), "");
        assert_eq!(
            AlphaTest::ONE_TENTH.discard("c.a"),
            "    if (!(c.a > 0.1)) discard;\n"
        );
        assert_eq!(
            AlphaTest::parse("NEVER 0").unwrap().discard("a"),
            "    discard;\n"
        );
    }
}
