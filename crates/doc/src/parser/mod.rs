//! The parser module.

use forge_fmt::{FormatterConfig, Visitable, Visitor};
use itertools::Itertools;
use solang_parser::{
    doccomment::{parse_doccomments, DocComment},
    pt::{
        Comment as SolangComment, EnumDefinition, ErrorDefinition, EventDefinition,
        FunctionDefinition, Identifier, Loc, SourceUnit, SourceUnitPart, StructDefinition,
        TypeDefinition, VariableDefinition,
    },
};

/// Parser error.
pub mod error;
use error::{ParserError, ParserResult};

/// Parser item.
mod item;
pub use item::{ParseItem, ParseSource};

/// Doc comment.
mod comment;
pub use comment::{Comment, CommentTag, Comments, CommentsRef};

/// The documentation parser. This type implements a [Visitor] trait.
///
/// While walking the parse tree, [Parser] will collect relevant source items and corresponding
/// doc comments. The resulting [ParseItem]s can be accessed by calling [Parser::items].
#[derive(Debug, Default)]
pub struct Parser {
    /// Initial comments from solang parser.
    comments: Vec<SolangComment>,
    /// Parser context.
    context: ParserContext,
    /// Parsed results.
    items: Vec<ParseItem>,
    /// Source file.
    source: String,
    /// The formatter config.
    fmt: FormatterConfig,
}

/// [Parser] context.
#[derive(Debug, Default)]
struct ParserContext {
    /// Current visited parent.
    parent: Option<ParseItem>,
    /// Current start pointer for parsing doc comments.
    doc_start_loc: usize,
}

impl Parser {
    /// Create a new instance of [Parser].
    pub fn new(comments: Vec<SolangComment>, source: String) -> Self {
        Self { comments, source, ..Default::default() }
    }

    /// Set formatter config on the [Parser]
    pub fn with_fmt(mut self, fmt: FormatterConfig) -> Self {
        self.fmt = fmt;
        self
    }

    /// Return the parsed items. Consumes the parser.
    pub fn items(self) -> Vec<ParseItem> {
        self.items
    }

    /// Visit the children elements with parent context.
    /// This function memoizes the previous parent, sets the context
    /// to a new one and invokes a visit function. The context will be reset
    /// to the previous parent at the end of the function.
    fn with_parent(
        &mut self,
        mut parent: ParseItem,
        mut visit: impl FnMut(&mut Self) -> ParserResult<()>,
    ) -> ParserResult<ParseItem> {
        let curr = self.context.parent.take();
        self.context.parent = Some(parent);
        visit(self)?;
        parent = self.context.parent.take().unwrap();
        self.context.parent = curr;
        Ok(parent)
    }

    /// Adds a child element to the parent item if it exists.
    /// Otherwise the element will be added to a top-level items collection.
    /// Moves the doc comment pointer to the end location of the child element.
    fn add_element_to_parent(&mut self, source: ParseSource, loc: Loc) -> ParserResult<()> {
        let child = self.new_item(source, loc.start())?;
        if let Some(parent) = self.context.parent.as_mut() {
            parent.children.push(child);
        } else {
            self.items.push(child);
        }
        self.context.doc_start_loc = loc.end();
        Ok(())
    }

    /// Create new [ParseItem] with comments and formatted code.
    fn new_item(&mut self, source: ParseSource, loc_start: usize) -> ParserResult<ParseItem> {
        let docs = self.parse_docs(loc_start)?;
        ParseItem::new(source).with_comments(docs).with_code(&self.source, self.fmt.clone())
    }

    /// Parse the doc comments from the current start location.
    fn parse_docs(&mut self, end: usize) -> ParserResult<Comments> {
        self.parse_docs_range(self.context.doc_start_loc, end)
    }

    /// Parse doc comments from the within specified range.
    fn parse_docs_range(&mut self, start: usize, end: usize) -> ParserResult<Comments> {
        let mut res = vec![];
        for comment in parse_doccomments(&self.comments, start, end) {
            match comment {
                DocComment::Line { comment } => res.push(comment),
                DocComment::Block { comments } => res.extend(comments),
            }
        }

        // Filter out `@solidity` and empty tags
        // See https://docs.soliditylang.org/en/v0.8.17/assembly.html#memory-safety
        let res = res
            .into_iter()
            .filter(|c| c.tag.trim() != "solidity" && !c.tag.trim().is_empty())
            .collect_vec();
        Ok(res.into())
    }
}

impl Visitor for Parser {
    type Error = ParserError;

    fn visit_source_unit(&mut self, source_unit: &mut SourceUnit) -> ParserResult<()> {
        for source in source_unit.0.iter_mut() {
            match source {
                SourceUnitPart::ContractDefinition(def) => {
                    // Create new contract parse item.
                    let contract =
                        self.new_item(ParseSource::Contract(def.clone()), def.loc.start())?;

                    // Move the doc pointer to the contract location start.
                    self.context.doc_start_loc = def.loc.start();

                    // Parse child elements with current contract as parent
                    let contract = self.with_parent(contract, |doc| {
                        def.parts
                            .iter_mut()
                            .map(|d| d.visit(doc))
                            .collect::<ParserResult<Vec<_>>>()?;
                        Ok(())
                    })?;

                    // Move the doc pointer to the contract location end.
                    self.context.doc_start_loc = def.loc.end();

                    // Add contract to the parsed items.
                    self.items.push(contract);
                }
                SourceUnitPart::FunctionDefinition(func) => self.visit_function(func)?,
                SourceUnitPart::EventDefinition(event) => self.visit_event(event)?,
                SourceUnitPart::ErrorDefinition(error) => self.visit_error(error)?,
                SourceUnitPart::StructDefinition(structure) => self.visit_struct(structure)?,
                SourceUnitPart::EnumDefinition(enumerable) => self.visit_enum(enumerable)?,
                SourceUnitPart::VariableDefinition(var) => self.visit_var_definition(var)?,
                SourceUnitPart::TypeDefinition(ty) => self.visit_type_definition(ty)?,
                _ => {}
            };
        }

        Ok(())
    }

    fn visit_enum(&mut self, enumerable: &mut EnumDefinition) -> ParserResult<()> {
        self.add_element_to_parent(ParseSource::Enum(enumerable.clone()), enumerable.loc)
    }

    fn visit_var_definition(&mut self, var: &mut VariableDefinition) -> ParserResult<()> {
        self.add_element_to_parent(ParseSource::Variable(var.clone()), var.loc)
    }

    fn visit_function(&mut self, func: &mut FunctionDefinition) -> ParserResult<()> {
        // If the function parameter doesn't have a name, try to set it with
        // `@custom:name` tag if any was provided
        let mut start_loc = func.loc.start();
        for (loc, param) in func.params.iter_mut() {
            if let Some(param) = param {
                if param.name.is_none() {
                    let docs = self.parse_docs_range(start_loc, loc.end())?;
                    let name_tag =
                        docs.iter().find(|c| c.tag == CommentTag::Custom("name".to_owned()));
                    if let Some(name_tag) = name_tag {
                        if let Some(name) = name_tag.value.trim().split(' ').next() {
                            param.name =
                                Some(Identifier { loc: Loc::Implicit, name: name.to_owned() })
                        }
                    }
                }
            }
            start_loc = loc.end();
        }

        self.add_element_to_parent(ParseSource::Function(func.clone()), func.loc)
    }

    fn visit_struct(&mut self, structure: &mut StructDefinition) -> ParserResult<()> {
        self.add_element_to_parent(ParseSource::Struct(structure.clone()), structure.loc)
    }

    fn visit_event(&mut self, event: &mut EventDefinition) -> ParserResult<()> {
        self.add_element_to_parent(ParseSource::Event(event.clone()), event.loc)
    }

    fn visit_error(&mut self, error: &mut ErrorDefinition) -> ParserResult<()> {
        self.add_element_to_parent(ParseSource::Error(error.clone()), error.loc)
    }

    fn visit_type_definition(&mut self, def: &mut TypeDefinition) -> ParserResult<()> {
        self.add_element_to_parent(ParseSource::Type(def.clone()), def.loc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solang_parser::parse;

    #[inline]
    fn parse_source(src: &str) -> Vec<ParseItem> {
        let (mut source, comments) = parse(src, 0).expect("failed to parse source");
        let mut doc = Parser::new(comments, src.to_owned());
        source.visit(&mut doc).expect("failed to visit source");
        doc.items()
    }

    macro_rules! test_single_unit {
        ($test:ident, $src:expr, $variant:ident $identity:expr) => {
            #[test]
            fn $test() {
                let items = parse_source($src);
                assert_eq!(items.len(), 1);
                let item = items.first().unwrap();
                assert!(item.comments.is_empty());
                assert!(item.children.is_empty());
                assert_eq!(item.source.ident(), $identity);
                assert!(matches!(item.source, ParseSource::$variant(_)));
            }
        };
    }

    #[test]
    fn empty_source() {
        assert_eq!(parse_source(""), vec![]);
    }

    test_single_unit!(single_function, "function someFn() { }", Function "someFn");
    test_single_unit!(single_variable, "uint256 constant VALUE = 0;", Variable "VALUE");
    test_single_unit!(single_event, "event SomeEvent();", Event "SomeEvent");
    test_single_unit!(single_error, "error SomeError();", Error "SomeError");
    test_single_unit!(single_struct, "struct SomeStruct { }", Struct "SomeStruct");
    test_single_unit!(single_enum, "enum SomeEnum { SOME, OTHER }", Enum "SomeEnum");
    test_single_unit!(single_contract, "contract Contract { }", Contract "Contract");

    #[test]
    fn multiple_shallow_contracts() {
        let items = parse_source(
            r"
            contract A { }
            contract B { }
            contract C { }
        ",
        );
        assert_eq!(items.len(), 3);

        let first_item = items.first().unwrap();
        assert!(matches!(first_item.source, ParseSource::Contract(_)));
        assert_eq!(first_item.source.ident(), "A");

        let first_item = items.get(1).unwrap();
        assert!(matches!(first_item.source, ParseSource::Contract(_)));
        assert_eq!(first_item.source.ident(), "B");

        let first_item = items.get(2).unwrap();
        assert!(matches!(first_item.source, ParseSource::Contract(_)));
        assert_eq!(first_item.source.ident(), "C");
    }

    #[test]
    fn contract_with_children_items() {
        let items = parse_source(
            r"
            event TopLevelEvent();

            contract Contract {
                event ContractEvent();
                error ContractError();
                struct ContractStruct { }
                enum ContractEnum { }

                uint256 constant CONTRACT_CONSTANT;
                bool contractVar;

                function contractFunction(uint256) external returns (uint256) {
                    bool localVar; // must be ignored
                }
            }
        ",
        );

        assert_eq!(items.len(), 2);

        let event = items.first().unwrap();
        assert!(event.comments.is_empty());
        assert!(event.children.is_empty());
        assert_eq!(event.source.ident(), "TopLevelEvent");
        assert!(matches!(event.source, ParseSource::Event(_)));

        let contract = items.get(1).unwrap();
        assert!(contract.comments.is_empty());
        assert_eq!(contract.children.len(), 7);
        assert_eq!(contract.source.ident(), "Contract");
        assert!(matches!(contract.source, ParseSource::Contract(_)));
        assert!(contract.children.iter().all(|ch| ch.children.is_empty()));
        assert!(contract.children.iter().all(|ch| ch.comments.is_empty()));
    }

    #[test]
    fn contract_with_fallback() {
        let items = parse_source(
            r"
            contract Contract {
                fallback() external payable {}
            }
        ",
        );

        assert_eq!(items.len(), 1);

        let contract = items.first().unwrap();
        assert!(contract.comments.is_empty());
        assert_eq!(contract.children.len(), 1);
        assert_eq!(contract.source.ident(), "Contract");
        assert!(matches!(contract.source, ParseSource::Contract(_)));

        let fallback = contract.children.first().unwrap();
        assert_eq!(fallback.source.ident(), "fallback");
        assert!(matches!(fallback.source, ParseSource::Function(_)));
    }

    #[test]
    fn contract_with_doc_comments() {
        let items = parse_source(
            r"
            pragma solidity ^0.8.19;
            /// @name Test
            ///  no tag
            ///@notice    Cool contract    
            ///   @  dev     This is not a dev tag 
            /**
             * @dev line one
             *    line 2
             */
            contract Test {
                /*** my function    
                      i like whitespace    
            */
                function test() {}
            }
        ",
        );

        assert_eq!(items.len(), 1);

        let contract = items.first().unwrap();
        assert_eq!(contract.comments.len(), 2);
        assert_eq!(
            *contract.comments.first().unwrap(),
            Comment::new(CommentTag::Notice, "Cool contract".to_owned())
        );
        assert_eq!(
            *contract.comments.get(1).unwrap(),
            Comment::new(CommentTag::Dev, "line one\nline 2".to_owned())
        );

        let function = contract.children.first().unwrap();
        assert_eq!(
            *function.comments.first().unwrap(),
            Comment::new(CommentTag::Notice, "my function\ni like whitespace".to_owned())
        );
    }
}
