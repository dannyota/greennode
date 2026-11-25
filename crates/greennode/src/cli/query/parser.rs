// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.

use super::lexer::{self, Kind, SyntaxError, Token};
use super::literal;
use crate::cli::render::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NodeKind {
    Empty,
    Comparator,
    CurrentNode,
    ExpRef,
    FunctionExpression,
    Field,
    FilterProjection,
    Flatten,
    Identity,
    Index,
    IndexExpression,
    KeyValPair,
    Literal,
    MultiSelectHash,
    MultiSelectList,
    OrExpression,
    AndExpression,
    NotExpression,
    Pipe,
    Projection,
    Subexpression,
    Slice,
    ValueProjection,
}

#[derive(Debug, PartialEq)]
pub(super) enum NodeValue {
    None,
    Text(String),
    Index(i64),
    Slice([Option<i64>; 3]),
    Comparator(Kind),
    Literal(Value),
}

#[derive(Debug, PartialEq)]
pub(super) struct Node {
    pub(super) kind: NodeKind,
    pub(super) value: NodeValue,
    pub(super) children: Vec<Node>,
    depth: usize,
}

fn node(
    kind: NodeKind,
    value: NodeValue,
    children: Vec<Node>,
    offset: usize,
) -> Result<Node, SyntaxError> {
    let depth = children
        .iter()
        .map(|child| child.depth)
        .max()
        .map_or(0, |n| n + 1);
    if depth > 64 {
        return Err(SyntaxError { offset });
    }
    Ok(Node {
        kind,
        value,
        children,
        depth,
    })
}

fn binding(kind: Kind) -> u8 {
    match kind {
        Kind::Pipe => 1,
        Kind::Or => 2,
        Kind::And => 3,
        Kind::Eq | Kind::Ne | Kind::Lt | Kind::Lte | Kind::Gt | Kind::Gte => 5,
        Kind::Flatten => 9,
        Kind::Star => 20,
        Kind::Filter => 21,
        Kind::Dot => 40,
        Kind::Not => 45,
        Kind::Lbrace => 50,
        Kind::Lbracket => 55,
        Kind::Lparen => 60,
        _ => 0,
    }
}

pub(super) fn parse(expression: &str) -> Result<Node, SyntaxError> {
    let tokens = lexer::tokenize(expression)?;
    let mut parser = Parser {
        tokens,
        index: 0,
        end: expression.len(),
    };
    let result = parser.expression(0, 0)?;
    if parser.current() != Kind::Eof {
        return Err(parser.error());
    }
    Ok(result)
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    end: usize,
}

impl Parser {
    fn current(&self) -> Kind {
        self.look(0)
    }
    fn look(&self, n: usize) -> Kind {
        self.index
            .checked_add(n)
            .and_then(|i| self.tokens.get(i))
            .map_or(Kind::Eof, |t| t.kind)
    }
    fn offset(&self) -> usize {
        self.tokens.get(self.index).map_or(self.end, |t| t.position)
    }
    fn error(&self) -> SyntaxError {
        SyntaxError {
            offset: self.offset(),
        }
    }
    fn advance(&mut self) {
        self.index = self.index.saturating_add(1);
    }
    fn take(&mut self, kind: Kind) -> Result<(), SyntaxError> {
        if self.current() != kind {
            return Err(self.error());
        }
        self.advance();
        Ok(())
    }
    fn make(
        &self,
        kind: NodeKind,
        value: NodeValue,
        children: Vec<Node>,
    ) -> Result<Node, SyntaxError> {
        node(kind, value, children, self.offset())
    }
    fn identity(&self) -> Result<Node, SyntaxError> {
        self.make(NodeKind::Identity, NodeValue::None, vec![])
    }
    fn descend(&mut self, bp: u8, depth: usize) -> Result<Node, SyntaxError> {
        if depth >= 64 {
            return Err(self.error());
        }
        self.expression(bp, depth + 1)
    }
    fn expression(&mut self, bp: u8, depth: usize) -> Result<Node, SyntaxError> {
        let kind = self.current();
        let value = self
            .tokens
            .get(self.index)
            .map_or_else(String::new, |t| t.value.clone());
        let offset = self.offset();
        self.advance();
        let mut left = self.nud(kind, value, offset, depth)?;
        while bp < binding(self.current()) {
            let op = self.current();
            self.advance();
            left = self.led(op, left, depth)?;
        }
        Ok(left)
    }
    fn nud(
        &mut self,
        kind: Kind,
        value: String,
        offset: usize,
        depth: usize,
    ) -> Result<Node, SyntaxError> {
        match kind {
            Kind::JsonLiteral => {
                let value = literal::parse(&value, offset)?;
                self.make(NodeKind::Literal, NodeValue::Literal(value), vec![])
            }
            Kind::StringLiteral => self.make(
                NodeKind::Literal,
                NodeValue::Literal(Value::String(value)),
                vec![],
            ),
            Kind::Expref => {
                let child = self.descend(binding(Kind::Expref), depth)?;
                self.make(NodeKind::ExpRef, NodeValue::None, vec![child])
            }
            Kind::Filter => {
                let identity = self.identity()?;
                self.filter(identity, depth)
            }
            Kind::Lbrace => self.multi_hash(depth),
            Kind::UnquotedIdentifier | Kind::QuotedIdentifier => {
                if kind == Kind::QuotedIdentifier && self.current() == Kind::Lparen {
                    return Err(SyntaxError { offset });
                }
                self.make(NodeKind::Field, NodeValue::Text(value), vec![])
            }
            Kind::Current => self.make(NodeKind::CurrentNode, NodeValue::None, vec![]),
            Kind::Not => {
                let child = self.descend(binding(Kind::Not), depth)?;
                self.make(NodeKind::NotExpression, NodeValue::None, vec![child])
            }
            Kind::Lparen => {
                let child = self.descend(0, depth)?;
                self.take(Kind::Rparen)?;
                Ok(child)
            }
            Kind::Star => {
                let left = self.identity()?;
                let right = if self.current() == Kind::Rbracket {
                    self.identity()?
                } else {
                    self.projection_rhs(binding(Kind::Star), depth)?
                };
                self.make(
                    NodeKind::ValueProjection,
                    NodeValue::None,
                    vec![left, right],
                )
            }
            Kind::Flatten => {
                let identity = self.identity()?;
                let left = self.make(NodeKind::Flatten, NodeValue::None, vec![identity])?;
                let right = self.projection_rhs(binding(Kind::Flatten), depth)?;
                self.make(NodeKind::Projection, NodeValue::None, vec![left, right])
            }
            Kind::Lbracket => {
                if matches!(self.current(), Kind::Number | Kind::Colon) {
                    let right = self.index_expression()?;
                    let left = self.identity()?;
                    self.project_slice(left, right, depth)
                } else if self.current() == Kind::Star && self.look(1) == Kind::Rbracket {
                    self.advance();
                    self.advance();
                    let left = self.identity()?;
                    let right = self.projection_rhs(binding(Kind::Star), depth)?;
                    self.make(NodeKind::Projection, NodeValue::None, vec![left, right])
                } else {
                    self.multi_list(depth)
                }
            }
            _ => Err(SyntaxError { offset }),
        }
    }
    fn led(&mut self, kind: Kind, left: Node, depth: usize) -> Result<Node, SyntaxError> {
        match kind {
            Kind::Filter => self.filter(left, depth),
            Kind::Lparen => {
                let mut args = Vec::new();
                while self.current() != Kind::Rparen {
                    args.push(self.descend(0, depth)?);
                    if self.current() == Kind::Comma {
                        self.advance();
                    }
                }
                self.take(Kind::Rparen)?;
                self.make(NodeKind::FunctionExpression, left.value, args)
            }
            Kind::Dot => {
                if self.current() == Kind::Star {
                    self.advance();
                    let right = self.projection_rhs(binding(Kind::Dot), depth)?;
                    self.make(
                        NodeKind::ValueProjection,
                        NodeValue::None,
                        vec![left, right],
                    )
                } else {
                    let right = self.dot_rhs(binding(Kind::Dot), depth)?;
                    self.make(NodeKind::Subexpression, NodeValue::None, vec![left, right])
                }
            }
            Kind::Pipe
            | Kind::Or
            | Kind::And
            | Kind::Eq
            | Kind::Ne
            | Kind::Lt
            | Kind::Lte
            | Kind::Gt
            | Kind::Gte => {
                let right = self.descend(binding(kind), depth)?;
                let (node_kind, value) = match kind {
                    Kind::Pipe => (NodeKind::Pipe, NodeValue::None),
                    Kind::Or => (NodeKind::OrExpression, NodeValue::None),
                    Kind::And => (NodeKind::AndExpression, NodeValue::None),
                    _ => (NodeKind::Comparator, NodeValue::Comparator(kind)),
                };
                self.make(node_kind, value, vec![left, right])
            }
            Kind::Flatten => {
                let left = self.make(NodeKind::Flatten, NodeValue::None, vec![left])?;
                let right = self.projection_rhs(binding(Kind::Flatten), depth)?;
                self.make(NodeKind::Projection, NodeValue::None, vec![left, right])
            }
            Kind::Lbracket => {
                if matches!(self.current(), Kind::Number | Kind::Colon) {
                    let right = self.index_expression()?;
                    self.project_slice(left, right, depth)
                } else {
                    self.take(Kind::Star)?;
                    self.take(Kind::Rbracket)?;
                    let right = self.projection_rhs(binding(Kind::Star), depth)?;
                    self.make(NodeKind::Projection, NodeValue::None, vec![left, right])
                }
            }
            _ => Err(self.error()),
        }
    }
    fn filter(&mut self, left: Node, depth: usize) -> Result<Node, SyntaxError> {
        let condition = self.descend(0, depth)?;
        self.take(Kind::Rbracket)?;
        let right = if self.current() == Kind::Flatten {
            self.identity()?
        } else {
            self.projection_rhs(binding(Kind::Filter), depth)?
        };
        self.make(
            NodeKind::FilterProjection,
            NodeValue::None,
            vec![left, right, condition],
        )
    }
    fn multi_list(&mut self, depth: usize) -> Result<Node, SyntaxError> {
        let mut children = Vec::new();
        loop {
            children.push(self.descend(0, depth)?);
            if self.current() == Kind::Rbracket {
                break;
            }
            self.take(Kind::Comma)?;
        }
        self.take(Kind::Rbracket)?;
        self.make(NodeKind::MultiSelectList, NodeValue::None, children)
    }
    fn multi_hash(&mut self, depth: usize) -> Result<Node, SyntaxError> {
        let mut children = Vec::new();
        loop {
            if !matches!(
                self.current(),
                Kind::UnquotedIdentifier | Kind::QuotedIdentifier
            ) {
                return Err(self.error());
            }
            let key = self
                .tokens
                .get(self.index)
                .map_or_else(String::new, |t| t.value.clone());
            self.advance();
            self.take(Kind::Colon)?;
            let value = self.descend(0, depth)?;
            children.push(self.make(NodeKind::KeyValPair, NodeValue::Text(key), vec![value])?);
            if self.current() == Kind::Comma {
                self.advance();
            } else if self.current() == Kind::Rbrace {
                self.advance();
                break;
            }
        }
        self.make(NodeKind::MultiSelectHash, NodeValue::None, children)
    }
    fn dot_rhs(&mut self, bp: u8, depth: usize) -> Result<Node, SyntaxError> {
        if matches!(
            self.current(),
            Kind::QuotedIdentifier | Kind::UnquotedIdentifier | Kind::Star
        ) {
            self.descend(bp, depth)
        } else if self.current() == Kind::Lbracket {
            self.advance();
            self.multi_list(depth)
        } else if self.current() == Kind::Lbrace {
            self.advance();
            self.multi_hash(depth)
        } else {
            Err(self.error())
        }
    }
    fn projection_rhs(&mut self, bp: u8, depth: usize) -> Result<Node, SyntaxError> {
        if binding(self.current()) < 10 {
            self.identity()
        } else if matches!(self.current(), Kind::Lbracket | Kind::Filter) {
            self.descend(bp, depth)
        } else if self.current() == Kind::Dot {
            self.advance();
            self.dot_rhs(bp, depth)
        } else {
            Err(self.error())
        }
    }
    fn index_expression(&mut self) -> Result<Node, SyntaxError> {
        if self.current() == Kind::Colon || self.look(1) == Kind::Colon {
            let mut parts = [None; 3];
            let mut index = 0;
            while self.current() != Kind::Rbracket && index < 3 {
                match self.current() {
                    Kind::Colon => {
                        index += 1;
                        self.advance();
                    }
                    Kind::Number => {
                        let number = self.number()?;
                        if let Some(slot) = parts.get_mut(index) {
                            *slot = Some(number);
                        } else {
                            return Err(self.error());
                        }
                    }
                    _ => return Err(self.error()),
                }
            }
            self.take(Kind::Rbracket)?;
            self.make(NodeKind::Slice, NodeValue::Slice(parts), vec![])
        } else {
            let number = self.number()?;
            self.take(Kind::Rbracket)?;
            self.make(NodeKind::Index, NodeValue::Index(number), vec![])
        }
    }
    fn number(&mut self) -> Result<i64, SyntaxError> {
        let value = self
            .tokens
            .get(self.index)
            .ok_or_else(|| self.error())?
            .value
            .parse::<i64>()
            .map_err(|_| self.error())?;
        self.advance();
        Ok(value)
    }
    fn project_slice(
        &mut self,
        left: Node,
        right: Node,
        depth: usize,
    ) -> Result<Node, SyntaxError> {
        let slice = right.kind == NodeKind::Slice;
        let index = self.make(
            NodeKind::IndexExpression,
            NodeValue::None,
            vec![left, right],
        )?;
        if slice {
            let right = self.projection_rhs(binding(Kind::Star), depth)?;
            self.make(NodeKind::Projection, NodeValue::None, vec![index, right])
        } else {
            Ok(index)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn field() {
        let node = parse("foo").unwrap();
        assert_eq!(node.kind, NodeKind::Field);
        assert_eq!(node.value, NodeValue::Text("foo".into()));
        assert_eq!(
            parse(r#""bar""#).unwrap().value,
            NodeValue::Text("bar".into())
        );
    }

    #[test]
    fn projection_tree_matches_upstream() {
        let root = parse("foo[*].bar.baz.qux").unwrap();
        assert_eq!(root.kind, NodeKind::Projection);
        assert_eq!(root.children[0].value, NodeValue::Text("foo".into()));
        let rhs = &root.children[1];
        assert_eq!(rhs.kind, NodeKind::Subexpression);
        assert_eq!(rhs.children[0].kind, NodeKind::Subexpression);
        assert_eq!(
            rhs.children[0].children[0].value,
            NodeValue::Text("bar".into())
        );
        assert_eq!(
            rhs.children[0].children[1].value,
            NodeValue::Text("baz".into())
        );
        assert_eq!(rhs.children[1].value, NodeValue::Text("qux".into()));
    }

    #[test]
    fn precedence_of_boolean_operators() {
        let root = parse("a||b&&c").unwrap();
        assert_eq!(root.kind, NodeKind::OrExpression);
        assert_eq!(root.children[1].kind, NodeKind::AndExpression);
        let root = parse("a&&b||c").unwrap();
        assert_eq!(root.children[0].kind, NodeKind::AndExpression);
    }

    #[test]
    fn precedence_of_pipe_and_comparisons() {
        let root = parse("a|b==c||d").unwrap();
        assert_eq!(root.kind, NodeKind::Pipe);
        assert_eq!(root.children[1].kind, NodeKind::OrExpression);
        assert_eq!(root.children[1].children[0].kind, NodeKind::Comparator);
        assert_eq!(
            parse("a==b|c").unwrap().children[0].kind,
            NodeKind::Comparator
        );
    }

    #[test]
    fn nesting_limit() {
        let expression = |n| format!("{}a{}", "(".repeat(n), ")".repeat(n));
        assert!(parse(&expression(64)).is_ok());
        assert!(parse(&expression(65)).is_err());
    }

    #[test]
    fn ast_depth_limit() {
        let expression = |n| vec!["a"; n].join(".");
        assert!(parse(&expression(65)).is_ok());
        assert!(parse(&expression(66)).is_err());
    }

    #[test]
    fn index_slice_and_projection_shapes() {
        let root = parse("foo[-1]").unwrap();
        assert_eq!(root.kind, NodeKind::IndexExpression);
        assert_eq!(root.children[1].value, NodeValue::Index(-1));
        let root = parse("foo[1:3:2]").unwrap();
        assert_eq!(root.kind, NodeKind::Projection);
        assert_eq!(
            root.children[0].children[1].value,
            NodeValue::Slice([Some(1), Some(3), Some(2)])
        );
        assert_eq!(parse("foo.*").unwrap().kind, NodeKind::ValueProjection);
        assert_eq!(parse("foo[]").unwrap().kind, NodeKind::Projection);
        assert_eq!(parse("!(@)").unwrap().kind, NodeKind::NotExpression);
    }

    #[test]
    fn parser_errors_have_positions() {
        for (input, offset) in [
            ("foo.", 4),
            ("]", 0),
            (")", 0),
            ("}", 0),
            ("foo..bar", 4),
            ("foo@", 3),
            (r#"foo."bar"#, 8),
            ("[*][", 4),
        ] {
            assert_eq!(parse(input).unwrap_err().offset, offset, "{input}");
        }
    }

    #[test]
    fn json_literal_depth_and_surrogates() {
        let expression = |n| format!("`{}0{}`", "[".repeat(n), "]".repeat(n));
        assert!(parse(&expression(64)).is_ok());
        assert!(parse(&expression(65)).is_err());
        assert_eq!(
            parse(r#"`"\ud800"`"#).unwrap().value,
            NodeValue::Literal(Value::String("�".into()))
        );
        assert!(parse(r#"`"\q"`"#).is_err());
        assert_eq!(
            parse("`9007199254740993`").unwrap().value,
            NodeValue::Literal(Value::Number(9007199254740992.0))
        );
    }

    #[test]
    fn filter_and_multiselect_shapes() {
        let root = parse("a[?b<=c]").unwrap();
        assert_eq!(root.kind, NodeKind::FilterProjection);
        assert_eq!(root.children[2].kind, NodeKind::Comparator);
        assert_eq!(root.children[2].value, NodeValue::Comparator(Kind::Lte));
        let root = parse("{a:b,c:d}").unwrap();
        assert_eq!(root.kind, NodeKind::MultiSelectHash);
        assert_eq!(root.children[0].value, NodeValue::Text("a".into()));
        assert_eq!(parse("[a,b]").unwrap().kind, NodeKind::MultiSelectList);
        assert_eq!(parse("f(@,)").unwrap().children.len(), 1);
        assert_eq!(parse("&a").unwrap().kind, NodeKind::ExpRef);
    }

    #[test]
    fn remaining_upstream_parser_errors() {
        for input in [
            "[foo",
            "{foo: bar",
            "{foo bar}",
            "[foo bar]",
            "&&&&&&&&&&&&t(",
        ] {
            assert!(parse(input).is_err(), "{input}");
        }
    }

    #[test]
    fn remaining_forms_compile() {
        for input in [
            "[?foo]",
            "{a:b}",
            "[a,b]",
            "f(a)",
            "`1`",
            "'x'",
            "&a",
            "length(@,)",
        ] {
            assert!(parse(input).is_ok(), "{input}");
        }
        assert!(parse("[a b]").is_err());
    }
}
