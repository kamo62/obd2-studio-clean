use crate::error::{CoreError, Result};

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    A,
    B,
    C,
    D,
    Signed,
    Plus,
    Minus,
    Star,
    Slash,
    LeftParen,
    RightParen,
}

/// A small expression evaluator for the formulas used by Mode 01 PIDs.
///
/// Supported syntax: `A`–`D`, decimal numbers, `+ - * /`, parentheses and
/// `signed(x)`, which interprets a 16-bit value as two's complement.
#[derive(Debug, Clone)]
pub struct Formula {
    tokens: Vec<Token>,
    required_bytes: u8,
}

impl Formula {
    pub fn parse(text: &str) -> Result<Self> {
        let tokens = tokenize(text)?;
        if tokens.is_empty() {
            return Err(CoreError::Invalid("empty PID formula".into()));
        }
        let mut parser = Parser {
            tokens: &tokens,
            position: 0,
            bytes: [0; 4],
        };
        parser.parse_expression()?;
        if parser.position != tokens.len() {
            return Err(CoreError::Invalid(format!(
                "unexpected token in PID formula: {text}"
            )));
        }
        let required_bytes = required_bytes(&tokens);
        Ok(Self {
            tokens,
            required_bytes,
        })
    }

    pub fn required_bytes(&self) -> u8 {
        self.required_bytes
    }

    pub fn evaluate(&self, data: &[u8]) -> Result<f64> {
        if data.len() < self.required_bytes as usize {
            return Err(CoreError::Invalid(format!(
                "PID has {} byte(s), formula needs {}",
                data.len(),
                self.required_bytes
            )));
        }
        let mut bytes = [0u8; 4];
        bytes[..data.len().min(4)].copy_from_slice(&data[..data.len().min(4)]);
        let mut parser = Parser {
            tokens: &self.tokens,
            position: 0,
            bytes,
        };
        parser.parse_expression()
    }
}

fn required_bytes(tokens: &[Token]) -> u8 {
    let mut required = 0;
    for token in tokens {
        let index = match token {
            Token::A => 1,
            Token::B => 2,
            Token::C => 3,
            Token::D => 4,
            _ => continue,
        };
        required = required.max(index);
    }
    required
}

fn tokenize(text: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;

    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        match ch {
            '+' => {
                tokens.push(Token::Plus);
                index += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                index += 1;
            }
            '*' => {
                tokens.push(Token::Star);
                index += 1;
            }
            '/' => {
                tokens.push(Token::Slash);
                index += 1;
            }
            '(' => {
                tokens.push(Token::LeftParen);
                index += 1;
            }
            ')' => {
                tokens.push(Token::RightParen);
                index += 1;
            }
            _ if ch.is_ascii_digit() || ch == '.' => {
                let start = index;
                let mut seen_dot = false;
                while index < chars.len() && (chars[index].is_ascii_digit() || chars[index] == '.')
                {
                    if chars[index] == '.' {
                        if seen_dot {
                            return Err(CoreError::Invalid(format!(
                                "malformed number in PID formula: {text}"
                            )));
                        }
                        seen_dot = true;
                    }
                    index += 1;
                }
                let value: String = chars[start..index].iter().collect();
                let parsed = value.parse::<f64>().map_err(|_| {
                    CoreError::Invalid(format!("malformed number {value} in PID formula"))
                })?;
                tokens.push(Token::Number(parsed));
            }
            _ if ch.is_ascii_alphabetic() => {
                let start = index;
                while index < chars.len() && chars[index].is_ascii_alphabetic() {
                    index += 1;
                }
                let name: String = chars[start..index].iter().collect();
                let token = match name.to_ascii_lowercase().as_str() {
                    "a" => Token::A,
                    "b" => Token::B,
                    "c" => Token::C,
                    "d" => Token::D,
                    "signed" => Token::Signed,
                    _ => {
                        return Err(CoreError::Invalid(format!(
                            "unknown identifier {name} in PID formula"
                        )));
                    }
                };
                tokens.push(token);
            }
            _ => {
                return Err(CoreError::Invalid(format!(
                    "unexpected character {ch} in PID formula"
                )));
            }
        }
    }

    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
    bytes: [u8; 4],
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn parse_expression(&mut self) -> Result<f64> {
        let mut value = self.parse_term()?;
        loop {
            match self.peek() {
                Some(Token::Plus) => {
                    self.position += 1;
                    value += self.parse_term()?;
                }
                Some(Token::Minus) => {
                    self.position += 1;
                    value -= self.parse_term()?;
                }
                _ => break,
            }
        }
        Ok(value)
    }

    fn parse_term(&mut self) -> Result<f64> {
        let mut value = self.parse_unary()?;
        loop {
            match self.peek() {
                Some(Token::Star) => {
                    self.position += 1;
                    value *= self.parse_unary()?;
                }
                Some(Token::Slash) => {
                    self.position += 1;
                    let divisor = self.parse_unary()?;
                    if divisor.abs() < f64::EPSILON {
                        return Err(CoreError::Invalid("division by zero in PID formula".into()));
                    }
                    value /= divisor;
                }
                _ => break,
            }
        }
        Ok(value)
    }

    fn parse_unary(&mut self) -> Result<f64> {
        match self.peek() {
            Some(Token::Minus) => {
                self.position += 1;
                Ok(-self.parse_unary()?)
            }
            Some(Token::Plus) => {
                self.position += 1;
                self.parse_unary()
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_atom(&mut self) -> Result<f64> {
        let token = self
            .peek()
            .cloned()
            .ok_or_else(|| CoreError::Invalid("unexpected end of PID formula".into()))?;
        self.position += 1;
        match token {
            Token::Number(value) => Ok(value),
            Token::A => Ok(self.bytes[0] as f64),
            Token::B => Ok(self.bytes[1] as f64),
            Token::C => Ok(self.bytes[2] as f64),
            Token::D => Ok(self.bytes[3] as f64),
            Token::LeftParen => {
                let value = self.parse_expression()?;
                self.expect(Token::RightParen)?;
                Ok(value)
            }
            Token::Signed => {
                self.expect(Token::LeftParen)?;
                let value = self.parse_expression()?;
                self.expect(Token::RightParen)?;
                Ok(two_complement_16(value))
            }
            other => Err(CoreError::Invalid(format!(
                "unexpected token {other:?} in PID formula"
            ))),
        }
    }

    fn expect(&mut self, expected: Token) -> Result<()> {
        if self.peek() == Some(&expected) {
            self.position += 1;
            Ok(())
        } else {
            Err(CoreError::Invalid(format!(
                "expected {expected:?} in PID formula"
            )))
        }
    }
}

fn two_complement_16(value: f64) -> f64 {
    let wrapped = (value as i64) & 0xFFFF;
    if wrapped >= 0x8000 {
        (wrapped - 0x1_0000) as f64
    } else {
        wrapped as f64
    }
}

pub fn format_number(value: f64) -> String {
    if !value.is_finite() {
        return "n/a".into();
    }
    if value.fract().abs() < 0.005 {
        format!("{value:.0}")
    } else if value.abs() < 10.0 {
        format!("{value:.2}")
    } else {
        format!("{value:.1}")
    }
}
