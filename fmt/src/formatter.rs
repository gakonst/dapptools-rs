//! A Solidity formatter

use std::fmt::Write;

use indent_write::fmt::IndentWriter;
use itertools::Itertools;
use solang_parser::pt::*;
use thiserror::Error;

use crate::{
    comments::{CommentStringExt, CommentWithMetadata, Comments},
    helpers,
    solang_ext::*,
    visit::{Visitable, Visitor},
};

#[derive(Error, Debug)]
pub enum FormatterError {
    #[error(transparent)]
    Fmt(#[from] std::fmt::Error),
    #[error(transparent)]
    Custom(Box<dyn std::error::Error>),
}

impl FormatterError {
    fn fmt() -> Self {
        Self::Fmt(std::fmt::Error)
    }
    fn custom(err: impl std::error::Error + 'static) -> Self {
        Self::Custom(Box::new(err))
    }
}

#[allow(unused_macros)]
macro_rules! format_err {
    ($msg:literal $(,)?) => {
        $crate::formatter::FormatterError::custom($msg.to_string())
    };
    ($err:expr $(,)?) => {
        $crate::formatter::FormatterError::custom($err)
    };
    ($fmt:expr, $($arg:tt)*) => {
        $crate::formatter::FormatterError::custom(format!($fmt, $($arg)*))
    };
}

#[allow(unused_macros)]
macro_rules! bail {
    ($msg:literal $(,)?) => {
        return Err($crate::formatter::format_err!($msg))
    };
    ($err:expr $(,)?) => {
        return Err($err)
    };
    ($fmt:expr, $($arg:tt)*) => {
        return Err($crate::formatter::format_err!($fmt, $(arg)*))
    };
}

pub type Result<T, E = FormatterError> = std::result::Result<T, E>;

/// Contains the config and rule set
#[derive(Debug, Clone)]
pub struct FormatterConfig {
    /// Maximum line length where formatter will try to wrap the line
    pub line_length: usize,
    /// Number of spaces per indentation level
    pub tab_width: usize,
    /// Print spaces between brackets
    pub bracket_spacing: bool,
}

impl Default for FormatterConfig {
    fn default() -> Self {
        FormatterConfig { line_length: 80, tab_width: 4, bracket_spacing: false }
    }
}

#[derive(Default, Clone)]
struct IndentGroup {
    skip_line: bool,
}

struct FormatBuffer<W: Sized> {
    indents: Vec<IndentGroup>,
    base_indent_len: usize,
    tab_width: usize,
    is_beginning_of_line: bool,
    last_indent: String,
    last_char: Option<char>,
    current_line_len: usize,
    w: W,
    restrict_to_single_line: bool,
}

impl<W: Sized> FormatBuffer<W> {
    fn new(w: W, tab_width: usize) -> Self {
        Self {
            w,
            tab_width,
            base_indent_len: 0,
            indents: vec![],
            current_line_len: 0,
            is_beginning_of_line: true,
            last_indent: String::new(),
            last_char: None,
            restrict_to_single_line: false,
        }
    }

    fn create_temp_buf(&self) -> FormatBuffer<String> {
        let mut new = FormatBuffer::new(String::new(), self.tab_width);
        new.base_indent_len = self.current_indent_len();
        new.last_indent = " ".repeat(self.last_indent_len().saturating_sub(new.base_indent_len));
        new.current_line_len = self.current_line_len();
        new.last_char = self.last_char;
        new.restrict_to_single_line = self.restrict_to_single_line;
        new
    }

    fn restrict_to_single_line(&mut self, restricted: bool) {
        self.restrict_to_single_line = restricted;
    }

    fn indent(&mut self, delta: usize) {
        self.indents.extend(std::iter::repeat(IndentGroup::default()).take(delta));
    }

    fn dedent(&mut self, delta: usize) {
        self.indents.truncate(self.indents.len() - delta);
    }

    fn level(&self) -> usize {
        self.indents.iter().filter(|i| !i.skip_line).count()
    }

    fn last_indent_group_skipped(&self) -> bool {
        self.indents.last().map(|i| i.skip_line).unwrap_or(false)
    }

    fn set_last_indent_group_skipped(&mut self, skip_line: bool) {
        if let Some(i) = self.indents.last_mut() {
            i.skip_line = skip_line
        }
    }

    fn last_indent_len(&self) -> usize {
        self.last_indent.len() + self.base_indent_len
    }

    fn current_indent_len(&self) -> usize {
        self.level() * self.tab_width
    }

    fn set_current_line_len(&mut self, len: usize) {
        self.current_line_len = len
    }

    fn current_line_len(&self) -> usize {
        self.current_line_len
    }

    fn is_beginning_of_line(&self) -> bool {
        self.is_beginning_of_line
    }

    fn start_group(&mut self) {
        self.indents.push(IndentGroup { skip_line: true });
    }

    fn end_group(&mut self) {
        self.indents.pop();
    }

    fn last_char(&self) -> Option<char> {
        self.last_char
    }
}

impl<W: Write> FormatBuffer<W> {
    fn write_raw(&mut self, s: impl AsRef<str>) -> std::fmt::Result {
        let mut lines = s.as_ref().lines().peekable();
        while let Some(line) = lines.next() {
            // remove the whitespace that covered by the base indent length (this is normally the
            // case with temporary buffers as this will be readded by the underlying IndentWriter
            // later on
            let line_start = line
                .char_indices()
                .take(self.base_indent_len)
                .take_while(|(_, ch)| ch.is_whitespace())
                .last()
                .map(|(idx, _)| idx + 1)
                .unwrap_or(0);
            let trimmed_line = &line[line_start..];
            if !trimmed_line.is_empty() {
                self.w.write_str(trimmed_line)?;
                self.is_beginning_of_line = false;
            }
            if lines.peek().is_some() {
                if self.restrict_to_single_line {
                    return Err(std::fmt::Error)
                }
                self.w.write_char('\n')?;
                self.is_beginning_of_line = true;
            }
        }
        Ok(())
    }
}

impl<W: Write> Write for FormatBuffer<W> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        if s.is_empty() {
            return Ok(())
        }
        let is_multiline = s.contains('\n');
        if is_multiline && self.restrict_to_single_line {
            return Err(std::fmt::Error)
        }

        let mut level = self.level();

        if self.is_beginning_of_line && !s.trim_start().is_empty() {
            let indent = " ".repeat(self.tab_width * level);
            self.w.write_str(&indent)?;
            self.last_indent = indent;
        }

        if self.last_indent_group_skipped() {
            level += 1;
        }
        let indent = " ".repeat(self.tab_width * level);
        IndentWriter::new_skip_initial(&indent, &mut self.w).write_str(s)?;

        if let Some(last_char) = s.chars().next_back() {
            self.last_char = Some(last_char);
        }

        if is_multiline {
            self.set_last_indent_group_skipped(false);
            self.last_indent = indent;
            self.is_beginning_of_line = s.ends_with('\n');
            if self.is_beginning_of_line {
                self.current_line_len = 0;
            } else {
                self.current_line_len = s.lines().last().unwrap().len();
            }
        } else {
            self.is_beginning_of_line = false;
            self.current_line_len += s.len();
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
struct Chunk {
    postfixes_before: Vec<CommentWithMetadata>,
    prefixes: Vec<CommentWithMetadata>,
    content: String,
    postfixes: Vec<CommentWithMetadata>,
}

impl From<String> for Chunk {
    fn from(string: String) -> Self {
        Chunk { content: string, ..Default::default() }
    }
}

impl From<&str> for Chunk {
    fn from(string: &str) -> Self {
        Chunk { content: string.to_owned(), ..Default::default() }
    }
}

// TODO: store context entities as references without copying
#[derive(Default)]
struct Context {
    contract: Option<ContractDefinition>,
    function: Option<FunctionDefinition>,
}

/// A Solidity formatter
pub struct Formatter<'a, W> {
    buf: FormatBuffer<&'a mut W>,
    source: &'a str,
    config: FormatterConfig,
    temp_bufs: Vec<FormatBuffer<String>>,
    context: Context,
    comments: Comments,
}

macro_rules! write_chunk {
    ($self:expr, $format_str:literal) => {{
        write_chunk!($self, $format_str,)
    }};
    ($self:expr, $format_str:literal, $($arg:tt)*) => {{
        $self.write_chunk(&format!($format_str, $($arg)*).into())
    }};
    ($self:expr, $loc:expr) => {{
        write_chunk!($self, $loc, "")
    }};
    ($self:expr, $loc:expr, $format_str:literal) => {{
        write_chunk!($self, $loc, $format_str,)
    }};
    ($self:expr, $loc:expr, $format_str:literal, $($arg:tt)*) => {{
        let chunk = $self.chunk_at($loc, None, format_args!($format_str, $($arg)*));
        $self.write_chunk(&chunk)
    }};
    ($self:expr, $loc:expr, $end_loc:expr, $format_str:literal) => {{
        write_chunk!($self, $loc, $end_loc, $format_str,)
    }};
    ($self:expr, $loc:expr, $end_loc:expr, $format_str:literal, $($arg:tt)*) => {{
        let chunk = $self.chunk_at($loc, Some($end_loc), format_args!($format_str, $($arg)*));
        $self.write_chunk(&chunk)
    }};
}

macro_rules! writeln_chunk {
    ($self:expr) => {{
        writeln_chunk!($self, "")
    }};
    ($self:expr, $format_str:literal) => {{
        writeln_chunk!($self, $format_str,)
    }};
    ($self:expr, $format_str:literal, $($arg:tt)*) => {{
        write_chunk!($self, "{}\n", format_args!($format_str, $($arg)*))
    }};
    ($self:expr, $loc:expr) => {{
        writeln_chunk!($self, $loc, "")
    }};
    ($self:expr, $loc:expr, $format_str:literal) => {{
        writeln_chunk!($self, $loc, $format_str,)
    }};
    ($self:expr, $loc:expr, $format_str:literal, $($arg:tt)*) => {{
        write_chunk!($self, $loc, "{}\n", format_args!($format_str, $($arg)*))
    }};
    ($self:expr, $loc:expr, $end_loc:expr, $format_str:literal) => {{
        writeln_chunk!($self, $loc, $end_loc, $format_str,)
    }};
    ($self:expr, $loc:expr, $end_loc:expr, $format_str:literal, $($arg:tt)*) => {{
        write_chunk!($self, $loc, $end_loc, "{}\n", format_args!($format_str, $($arg)*))
    }};
}

macro_rules! buf_fn {
    ($vis:vis fn $name:ident(&self $(,)? $($arg_name:ident : $arg_ty:ty),*) $(-> $ret:ty)?) => {
        $vis fn $name(&self, $($arg_name : $arg_ty),*) $(-> $ret)? {
            if self.temp_bufs.is_empty() {
                self.buf.$name($($arg_name),*)
            } else {
                self.temp_bufs.last().unwrap().$name($($arg_name),*)
            }
        }
    };
    ($vis:vis fn $name:ident(&mut self $(,)? $($arg_name:ident : $arg_ty:ty),*) $(-> $ret:ty)?) => {
        $vis fn $name(&mut self, $($arg_name : $arg_ty),*) $(-> $ret)? {
            if self.temp_bufs.is_empty() {
                self.buf.$name($($arg_name),*)
            } else {
                self.temp_bufs.last_mut().unwrap().$name($($arg_name),*)
            }
        }
    };
}

struct Transaction<'f, 'a, W> {
    fmt: &'f mut Formatter<'a, W>,
    buffer: String,
    comments: Comments,
}

impl<'f, 'a, W> std::ops::Deref for Transaction<'f, 'a, W> {
    type Target = Formatter<'a, W>;
    fn deref(&self) -> &Self::Target {
        self.fmt
    }
}

impl<'f, 'a, W> std::ops::DerefMut for Transaction<'f, 'a, W> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.fmt
    }
}

impl<'f, 'a, W: Write> Transaction<'f, 'a, W> {
    fn new(
        fmt: &'f mut Formatter<'a, W>,
        mut fun: impl FnMut(&mut Formatter<'a, W>) -> Result<()>,
    ) -> Result<Self> {
        let mut comments = fmt.comments.clone();
        let buffer = fmt.with_temp_buf(|fmt| fun(fmt))?.w;
        comments = std::mem::replace(&mut fmt.comments, comments);
        Ok(Self { fmt, buffer, comments })
    }

    fn commit(self) -> Result<String> {
        self.fmt.comments = self.comments;
        write_chunk!(self.fmt, "{}", self.buffer)?;
        Ok(self.buffer)
    }
}

impl<'a, W: Write> Formatter<'a, W> {
    pub fn new(w: &'a mut W, source: &'a str, comments: Comments, config: FormatterConfig) -> Self {
        Self {
            buf: FormatBuffer::new(w, config.tab_width),
            source,
            config,
            temp_bufs: Vec::new(),
            context: Context::default(),
            comments,
        }
    }

    fn buf(&mut self) -> &mut dyn Write {
        if self.temp_bufs.is_empty() {
            &mut self.buf as &mut dyn Write
        } else {
            self.temp_bufs.last_mut().unwrap() as &mut dyn Write
        }
    }

    buf_fn! { fn indent(&mut self, delta: usize) }
    buf_fn! { fn dedent(&mut self, delta: usize) }
    buf_fn! { fn start_group(&mut self) }
    buf_fn! { fn end_group(&mut self) }
    buf_fn! { fn create_temp_buf(&self) -> FormatBuffer<String> }
    buf_fn! { fn restrict_to_single_line(&mut self, restricted: bool) }
    buf_fn! { fn set_current_line_len(&mut self, len: usize) }
    buf_fn! { fn current_line_len(&self) -> usize }
    buf_fn! { fn last_indent_len(&self) -> usize }
    buf_fn! { fn is_beginning_of_line(&self) -> bool }
    buf_fn! { fn last_char(&self) -> Option<char> }
    buf_fn! { fn last_indent_group_skipped(&self) -> bool }
    buf_fn! { fn set_last_indent_group_skipped(&mut self, skip: bool) }
    buf_fn! { fn write_raw(&mut self, s: impl AsRef<str>) -> std::fmt::Result }

    fn with_temp_buf(
        &mut self,
        mut fun: impl FnMut(&mut Self) -> Result<()>,
    ) -> Result<FormatBuffer<String>> {
        self.temp_bufs.push(self.create_temp_buf());
        let res = fun(self);
        let out = self.temp_bufs.pop().unwrap();
        res?;
        Ok(out)
    }

    /// Does the next written character require whitespace before
    fn next_chunk_needs_space(&self, next_char: char) -> bool {
        if self.is_beginning_of_line() {
            return false
        }
        let last_char =
            if let Some(last_char) = self.last_char() { last_char } else { return false };
        if last_char.is_whitespace() {
            return false
        }
        if next_char.is_whitespace() {
            return false
        }
        match last_char {
            '{' | '[' => match next_char {
                '{' | '[' | '(' => false,
                '/' => true,
                _ => self.config.bracket_spacing,
            },
            '(' | '.' => false,
            '/' => true,
            _ => match next_char {
                '}' | ']' => self.config.bracket_spacing,
                ')' => false,
                '.' => false,
                _ => true,
            },
        }
    }

    /// Is length of the `text` with respect to already written line <= `config.line_length`
    fn will_it_fit(&self, text: impl AsRef<str>) -> bool {
        let text = text.as_ref();
        if text.is_empty() {
            return true
        }
        if text.contains('\n') {
            return false
        }
        let space = if self.next_chunk_needs_space(text.chars().next().unwrap()) { 1 } else { 0 };
        self.config.line_length >=
            self.last_indent_len()
                .saturating_add(self.current_line_len())
                .saturating_add(text.len() + space)
    }

    /// Write empty brackets with respect to `config.bracket_spacing` setting:
    /// `"{ }"` if `true`, `"{}"` if `false`
    fn write_empty_brackets(&mut self) -> Result<()> {
        let brackets = if self.config.bracket_spacing { "{ }" } else { "{}" };
        write_chunk!(self, "{brackets}")?;
        Ok(())
    }

    /// Write semicolon to the buffer
    fn write_semicolon(&mut self) -> Result<()> {
        write!(self.buf(), ";")?;
        Ok(())
    }

    /// Write whitespace separator to the buffer
    /// `"\n"` if `multiline` is `true`, `" "` if `false`
    fn write_whitespace_separator(&mut self, multiline: bool) -> Result<()> {
        if !self.is_beginning_of_line() {
            write!(self.buf(), "{}", if multiline { "\n" } else { " " })?;
        }
        Ok(())
    }

    /// Returns number of blank lines between two LOCs
    fn blank_lines(&self, start: usize, end: usize) -> usize {
        self.source[start..end].matches('\n').count()
    }

    fn find_next_in_src(&self, byte_offset: usize, needle: char) -> Option<usize> {
        self.source[byte_offset..]
            .non_comment_chars()
            .position(|ch| needle == ch)
            .map(|p| byte_offset + p)
    }

    fn chunk_at(
        &mut self,
        byte_offset: usize,
        next_byte_offset: Option<usize>,
        content: impl std::fmt::Display,
    ) -> Chunk {
        Chunk {
            postfixes_before: self.comments.remove_postfixes_before(byte_offset),
            prefixes: self.comments.remove_prefixes_before(byte_offset),
            content: content.to_string(),
            postfixes: next_byte_offset
                .map(|byte_offset| self.comments.remove_postfixes_before(byte_offset))
                .unwrap_or_default(),
        }
    }

    fn chunked(
        &mut self,
        byte_offset: usize,
        next_byte_offset: Option<usize>,
        fun: impl FnMut(&mut Self) -> Result<()>,
    ) -> Result<Chunk> {
        let postfixes_before = self.comments.remove_postfixes_before(byte_offset);
        let prefixes = self.comments.remove_prefixes_before(byte_offset);
        let content = self.with_temp_buf(fun)?.w;
        let postfixes = next_byte_offset
            .map(|byte_offset| self.comments.remove_postfixes_before(byte_offset))
            .unwrap_or_default();
        Ok(Chunk { postfixes_before, prefixes, content, postfixes })
    }

    fn visit_to_chunk(
        &mut self,
        byte_offset: usize,
        next_byte_offset: Option<usize>,
        visitable: &mut impl Visitable,
    ) -> Result<Chunk> {
        self.chunked(byte_offset, next_byte_offset, |fmt| {
            visitable.visit(fmt)?;
            Ok(())
        })
    }

    /// Transform [Visitable] items to the list of chunks
    fn items_to_chunks<'b>(
        &mut self,
        next_byte_offset: Option<usize>,
        items: impl IntoIterator<Item = Result<(Loc, &'b mut (impl Visitable + 'b))>> + 'b,
    ) -> Result<Vec<Chunk>> {
        let mut items = items.into_iter().collect::<Result<Vec<_>>>()?.into_iter().peekable();
        let mut out = Vec::new();
        while let Some((loc, item)) = items.next() {
            let chunk_next_byte_offset =
                items.peek().map(|(loc, _)| loc.start()).or(next_byte_offset);
            out.push(self.visit_to_chunk(loc.start(), chunk_next_byte_offset, item)?);
        }
        Ok(out)
    }

    fn items_to_chunks_sorted<'b>(
        &mut self,
        next_byte_offset: Option<usize>,
        items: impl IntoIterator<Item = Result<(Loc, &'b mut (impl Visitable + AttrSortKey + 'b))>> + 'b,
    ) -> Result<Vec<Chunk>> {
        let mut items = items
            .into_iter()
            .map_ok(|(loc, vis)| (vis.attr_sort_key(), loc, vis))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .peekable();
        let mut out = Vec::new();
        while let Some((attr_sort_key, loc, item)) = items.next() {
            let chunk_next_byte_offset =
                items.peek().map(|(_, loc, _)| loc.start()).or(next_byte_offset);
            out.push((
                (attr_sort_key, loc),
                self.visit_to_chunk(loc.start(), chunk_next_byte_offset, item)?,
            ));
        }
        out.sort_by_key(|(k, _)| *k);
        Ok(out.into_iter().map(|(_, c)| c).collect_vec())
    }

    fn write_comment(&mut self, comment: &CommentWithMetadata) -> Result<()> {
        if comment.is_prefix() {
            let last_indent_group_skipped = self.last_indent_group_skipped();
            if !self.is_beginning_of_line() {
                writeln!(self.buf())?;
                self.set_last_indent_group_skipped(last_indent_group_skipped);
            }
            let mut lines = comment.comment.splitn(2, '\n');
            write!(self.buf(), "{}", lines.next().unwrap())?;
            if let Some(line) = lines.next() {
                writeln!(self.buf())?;
                self.set_last_indent_group_skipped(last_indent_group_skipped);
                self.write_raw(line)?;
            }
            writeln!(self.buf())?;
            self.set_last_indent_group_skipped(last_indent_group_skipped);
        } else {
            let indented = self.is_beginning_of_line();
            self.indented_if(indented, 1, |fmt| {
                if !indented && fmt.next_chunk_needs_space('/') {
                    write!(fmt.buf(), " ")?;
                }
                let mut lines = comment.comment.splitn(2, '\n');
                write!(fmt.buf(), "{}", lines.next().unwrap())?;
                if let Some(line) = lines.next() {
                    writeln!(fmt.buf())?;
                    fmt.write_raw(line)?;
                }
                if comment.is_line() {
                    writeln!(fmt.buf())?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    fn will_chunk_fit(&mut self, format_string: &str, chunk: &Chunk) -> Result<bool> {
        if let Some(chunk_str) = self.simulate_to_single_line(|fmt| fmt.write_chunk(chunk))? {
            Ok(self.will_it_fit(format_string.replacen("{}", &chunk_str, 1)))
        } else {
            Ok(false)
        }
    }

    fn are_chunks_separated_multiline<'b>(
        &mut self,
        format_string: &str,
        items: impl IntoIterator<Item = &'b Chunk>,
        separator: &str,
    ) -> Result<bool> {
        let items = items.into_iter().collect_vec();
        if let Some(chunks) = self.simulate_to_single_line(|fmt| {
            fmt.write_chunks_separated(items.iter().copied(), separator, false)
        })? {
            Ok(!self.will_it_fit(format_string.replacen("{}", &chunks, 1)))
        } else {
            Ok(true)
        }
    }

    fn write_postfix_comments_before(&mut self, byte_end: usize) -> Result<()> {
        for postfix in self.comments.remove_postfixes_before(byte_end) {
            self.write_comment(&postfix)?;
        }
        Ok(())
    }

    fn write_prefix_comments_before(&mut self, byte_end: usize) -> Result<()> {
        for prefix in self.comments.remove_prefixes_before(byte_end) {
            self.write_comment(&prefix)?;
        }
        Ok(())
    }

    fn write_chunk(&mut self, chunk: &Chunk) -> Result<()> {
        // handle comments before chunk
        for comment in &chunk.postfixes_before {
            self.write_comment(comment)?;
        }
        for comment in &chunk.prefixes {
            self.write_comment(comment)?;
        }

        // trim chunk start
        let content = if chunk.content.starts_with('\n') {
            let mut chunk = chunk.content.trim_start().to_string();
            chunk.insert(0, '\n');
            chunk
        } else if chunk.content.starts_with(' ') {
            let mut chunk = chunk.content.trim_start().to_string();
            chunk.insert(0, ' ');
            chunk
        } else {
            chunk.content.clone()
        };

        if !content.is_empty() {
            // add whitespace if necessary
            if self.next_chunk_needs_space(content.chars().next().unwrap()) {
                if self.will_it_fit(&content) {
                    write!(self.buf(), " ")?;
                } else {
                    writeln!(self.buf())?;
                }
            }

            // write chunk
            write!(self.buf(), "{content}")?;
        }

        // write any postfix comments
        for comment in &chunk.postfixes {
            self.write_comment(comment)?;
        }

        Ok(())
    }

    fn write_chunks_separated<'b>(
        &mut self,
        chunks: impl IntoIterator<Item = &'b Chunk>,
        separator: &str,
        multiline: bool,
    ) -> Result<()> {
        let mut chunks = chunks.into_iter().peekable();
        while let Some(chunk) = chunks.next() {
            let mut chunk = chunk.clone();

            // handle postfixes before and add newline if necessary
            let postfixes_before = std::mem::take(&mut chunk.postfixes_before);
            for comment in postfixes_before {
                self.write_comment(&comment)?;
            }
            if multiline && !self.is_beginning_of_line() {
                writeln!(self.buf())?;
            }

            // remove postfixes so we can add separator between
            let postfixes = std::mem::take(&mut chunk.postfixes);

            self.write_chunk(&chunk)?;

            // add separator
            if chunks.peek().is_some() {
                write!(self.buf(), "{}", separator)?;
                for comment in postfixes {
                    self.write_comment(&comment)?;
                }
                if multiline && !self.is_beginning_of_line() {
                    writeln!(self.buf())?;
                }
            } else {
                for comment in postfixes {
                    self.write_comment(&comment)?;
                }
            }
        }
        Ok(())
    }

    fn indented(&mut self, delta: usize, fun: impl FnMut(&mut Self) -> Result<()>) -> Result<()> {
        self.indented_if(true, delta, fun)
    }

    fn indented_if(
        &mut self,
        condition: bool,
        delta: usize,
        mut fun: impl FnMut(&mut Self) -> Result<()>,
    ) -> Result<()> {
        if condition {
            self.indent(delta);
        }
        let res = fun(self);
        if condition {
            self.dedent(delta);
        }
        res?;
        Ok(())
    }

    fn grouped(&mut self, mut fun: impl FnMut(&mut Self) -> Result<()>) -> Result<bool> {
        self.start_group();
        let res = fun(self);
        let indented = !self.last_indent_group_skipped();
        self.end_group();
        res?;
        Ok(indented)
    }

    fn transact<'b>(
        &'b mut self,
        fun: impl FnMut(&mut Self) -> Result<()>,
    ) -> Result<Transaction<'b, 'a, W>> {
        Transaction::new(self, fun)
    }

    fn simulate_to_string(&mut self, fun: impl FnMut(&mut Self) -> Result<()>) -> Result<String> {
        Ok(self.transact(fun)?.buffer)
    }

    fn chunk_to_string(&mut self, chunk: &Chunk) -> Result<String> {
        self.simulate_to_string(|fmt| fmt.write_chunk(chunk))
    }

    fn simulate_to_single_line(
        &mut self,
        mut fun: impl FnMut(&mut Self) -> Result<()>,
    ) -> Result<Option<String>> {
        let mut single_line = false;
        let tx = self.transact(|fmt| {
            fmt.restrict_to_single_line(true);
            single_line = match fun(fmt) {
                Ok(()) => true,
                Err(FormatterError::Fmt(_)) => false,
                Err(err) => bail!(err),
            };
            Ok(())
        })?;
        Ok(if single_line && tx.will_it_fit(&tx.buffer) { Some(tx.buffer) } else { None })
    }

    fn try_on_single_line(&mut self, mut fun: impl FnMut(&mut Self) -> Result<()>) -> Result<bool> {
        let mut single_line = false;
        let tx = self.transact(|fmt| {
            fmt.restrict_to_single_line(true);
            single_line = match fun(fmt) {
                Ok(()) => true,
                Err(FormatterError::Fmt(_)) => false,
                Err(err) => bail!(err),
            };
            Ok(())
        })?;
        Ok(if single_line && tx.will_it_fit(&tx.buffer) {
            tx.commit()?;
            true
        } else {
            false
        })
    }

    fn surrounded(
        &mut self,
        byte_offset: usize,
        first_chunk: impl std::fmt::Display,
        last_chunk: impl std::fmt::Display,
        next_byte_end: Option<usize>,
        mut fun: impl FnMut(&mut Self, bool) -> Result<()>,
    ) -> Result<()> {
        self.write_postfix_comments_before(byte_offset)?;

        write_chunk!(self, byte_offset, "{first_chunk}")?;

        let multiline = !self.try_on_single_line(|fmt| {
            fun(fmt, false)?;
            write_chunk!(fmt, byte_offset, "{last_chunk}")?;
            Ok(())
        })?;

        if multiline {
            self.indented(1, |fmt| {
                let contents = fmt
                    .with_temp_buf(|fmt| {
                        fmt.set_current_line_len(0);
                        fun(fmt, true)
                    })?
                    .w;
                if contents.chars().next().map(|ch| !ch.is_whitespace()).unwrap_or(false) {
                    fmt.write_whitespace_separator(true)?;
                }
                write_chunk!(fmt, "{contents}")
            })?;
            if let Some(next_byte_end) = next_byte_end {
                self.write_postfix_comments_before(next_byte_end)?;
            }
            let last_chunk = last_chunk.to_string();
            if !last_chunk.trim_start().is_empty() {
                self.write_whitespace_separator(true)?;
            }
            write_chunk!(self, byte_offset, "{last_chunk}")?;
        } else if let Some(next_byte_end) = next_byte_end {
            self.write_postfix_comments_before(next_byte_end)?;
        }

        Ok(())
    }

    fn write_lined_visitable<'b, I, V, F>(&mut self, items: I, needs_space_fn: F) -> Result<()>
    where
        I: Iterator<Item = &'b mut V> + 'b,
        V: Visitable + LineOfCode + 'b,
        F: Fn(&Option<&V>, &V) -> bool,
    {
        let mut last_item: Option<&V> = None;
        let mut last_byte_written = 0;
        let mut is_first_line = true;
        let mut items_iter = items.peekable();

        while let Some(item) = items_iter.next() {
            // check if the next block requires space
            let mut needs_space = needs_space_fn(&last_item, item);

            // write prefix comments
            let comments = self.comments.remove_prefixes_before(item.loc().start());
            for comment in &comments {
                if !is_first_line && (needs_space || comment.has_newline_before) {
                    writeln!(self.buf())?;
                    needs_space = false;
                }
                self.write_comment(comment)?;
                is_first_line = false;
                last_byte_written = comment.loc.end();
            }

            // write space if required or if there are blank lines in between
            if !is_first_line &&
                (needs_space || self.blank_lines(last_byte_written, item.loc().start()) > 1)
            {
                writeln!(self.buf())?;
            }

            // write source unit part
            item.visit(self)?;
            last_byte_written = item.loc().end();
            last_item = Some(item);
            is_first_line = false;

            // write postfix comments
            if let Some(next_item) = items_iter.peek() {
                let comments = self.comments.remove_postfixes_before(next_item.loc().start());
                for comment in comments {
                    self.write_comment(&comment)?;
                    last_byte_written = comment.loc.end();
                }
                self.write_whitespace_separator(true)?;
            }
        }
        Ok(())
    }

    fn visit_flattened_expr<E>(&mut self, flattened: &mut [FlatExpression<&mut E>]) -> Result<()>
    where
        E: Visitable + LineOfCode,
    {
        if !self.try_on_single_line(|fmt| fmt.visit_flattened_expr_multiline(flattened, false))? {
            self.grouped(|fmt| fmt.visit_flattened_expr_multiline(flattened, true))?;
        }
        Ok(())
    }

    fn visit_flattened_expr_multiline<E>(
        &mut self,
        flattened: &mut [FlatExpression<&mut E>],
        multiline: bool,
    ) -> Result<()>
    where
        E: Visitable + LineOfCode,
    {
        let mut flattened = flattened.iter_mut().peekable();
        let mut prefix_ops = String::new();
        while let Some(item) = flattened.next() {
            let loc = item.loc();
            match item {
                FlatExpression::Expression(expr) => {
                    let mut chunk = self.visit_to_chunk(expr.loc().end(), None, *expr)?;

                    chunk.content.insert_str(0, &prefix_ops);
                    prefix_ops.clear();
                    while let Some(FlatExpression::UnspacedOp(op)) = flattened.peek() {
                        chunk.content.push_str(op);
                        flattened.next();
                    }

                    self.write_chunk(&chunk)?;
                    if multiline && flattened.peek().is_some() {
                        self.write_whitespace_separator(true)?;
                    }
                }
                FlatExpression::SpacedOp(op) => {
                    write_chunk!(self, "{op}")?;
                }
                FlatExpression::UnspacedOp(op) => {
                    prefix_ops.push_str(op);
                }
                FlatExpression::Group(group) => {
                    let mut prefix = std::mem::take(&mut prefix_ops);
                    prefix.push('(');
                    let mut suffix = ")".to_string();
                    while let Some(FlatExpression::UnspacedOp(op)) = flattened.peek() {
                        suffix.push_str(op);
                        flattened.next();
                    }
                    self.surrounded(
                        loc.map(|loc| loc.start()).unwrap_or(0),
                        prefix,
                        suffix,
                        loc.map(|loc| loc.end()),
                        |fmt, _multiline| fmt.visit_flattened_expr(group),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn visit_assignment(&mut self, expr: &mut Expression) -> Result<()> {
        use Expression::*;

        if self.try_on_single_line(|fmt| expr.visit(fmt))? {
            return Ok(())
        }

        self.write_postfix_comments_before(expr.loc().start())?;
        self.write_prefix_comments_before(expr.loc().start())?;

        if self.is_beginning_of_line() &&
            self.try_on_single_line(|fmt| fmt.indented(1, |fmt| expr.visit(fmt)))?
        {
            return Ok(())
        } else {
            let mut fit_on_next_line = false;
            self.indented(1, |fmt| {
                let tx = fmt.transact(|fmt| {
                    writeln!(fmt.buf())?;
                    fit_on_next_line = fmt.try_on_single_line(|fmt| expr.visit(fmt))?;
                    Ok(())
                })?;
                if fit_on_next_line {
                    tx.commit()?;
                }
                Ok(())
            })?;
            if fit_on_next_line {
                return Ok(())
            }
        }

        let unsplittable = matches!(
            expr,
            BoolLiteral(..) |
                NumberLiteral(..) |
                RationalNumberLiteral(..) |
                HexNumberLiteral(..) |
                StringLiteral(..) |
                HexLiteral(..) |
                AddressLiteral(..) |
                Variable(..) |
                Unit(..) |
                This(..)
        );
        if unsplittable {
            self.indented(1, |fmt| expr.visit(fmt))?;
            return Ok(())
        }

        expr.visit(self)?;

        Ok(())
    }
}

// Traverse the Solidity Parse Tree and write to the code formatter
impl<'a, W: Write> Visitor for Formatter<'a, W> {
    type Error = FormatterError;

    fn visit_source(&mut self, loc: Loc) -> Result<()> {
        let source = String::from_utf8(self.source.as_bytes()[loc.start()..loc.end()].to_vec())
            .map_err(FormatterError::custom)?;
        let mut lines = source.splitn(2, '\n');

        write_chunk!(self, loc.start(), "{}", lines.next().unwrap())?;
        if let Some(remainder) = lines.next() {
            // Call with `self.write_str` and not `write!`, so we can have `\n` at the beginning
            // without triggering an indentation
            self.write_raw(&format!("\n{remainder}"))?;
        }

        let _ = self.comments.remove_comments_before(loc.end());

        Ok(())
    }

    fn visit_source_unit(&mut self, source_unit: &mut SourceUnit) -> Result<()> {
        // TODO: do we need to put pragma and import directives at the top of the file?
        // source_unit.0.sort_by_key(|item| match item {
        //     SourceUnitPart::PragmaDirective(_, _, _) => 0,
        //     SourceUnitPart::ImportDirective(_, _) => 1,
        //     _ => usize::MAX,
        // });

        self.write_lined_visitable(source_unit.0.iter_mut(), |last_unit, unit| match last_unit {
            Some(last_unit) => match last_unit {
                SourceUnitPart::ImportDirective(_) => {
                    !matches!(unit, SourceUnitPart::ImportDirective(_))
                }
                SourceUnitPart::ErrorDefinition(_) => {
                    !matches!(unit, SourceUnitPart::ErrorDefinition(_))
                }
                SourceUnitPart::Using(_) => !matches!(unit, SourceUnitPart::Using(_)),
                SourceUnitPart::VariableDefinition(_) => {
                    !matches!(unit, SourceUnitPart::VariableDefinition(_))
                }
                SourceUnitPart::DocComment(_) => false,
                _ => true,
            },
            None => false,
        })?;

        let comments = self.simulate_to_string(|fmt| {
            fmt.write_postfix_comments_before(fmt.source.len())?;
            fmt.write_prefix_comments_before(fmt.source.len())?;
            Ok(())
        })?;
        self.comments.remove_comments_before(self.source.len());
        write_chunk!(self, self.source.len(), "{}", comments.trim_end())?;

        Ok(())
    }

    fn visit_doc_comment(&mut self, doc_comment: &mut DocComment) -> Result<()> {
        match doc_comment.ty {
            CommentType::Line => {
                write!(self.buf(), "///{}", doc_comment.comment)?;
            }
            CommentType::Block => {
                let lines = doc_comment
                    .comment
                    .trim_end()
                    .lines()
                    .map(|line| line.trim_start())
                    .peekable()
                    .collect::<Vec<_>>();
                if lines.iter().skip(1).all(|line| line.starts_with('*')) {
                    writeln!(self.buf(), "/**")?;
                    let mut lines = lines.into_iter();
                    if let Some(first_line) = lines.next() {
                        if !first_line.is_empty() {
                            // write the original first line
                            writeln!(
                                self.buf(),
                                " *{}",
                                doc_comment.comment.lines().next().unwrap()
                            )?;
                        }
                    }
                    for line in lines {
                        writeln!(self.buf(), " *{}", &line[1..])?;
                    }
                    write!(self.buf(), " */")?;
                } else {
                    write!(self.buf(), "/**")?;
                    self.write_raw(&doc_comment.comment)?;
                    write!(self.buf(), "*/")?;
                }
            }
        }

        Ok(())
    }

    fn visit_contract(&mut self, contract: &mut ContractDefinition) -> Result<()> {
        self.context.contract = Some(contract.clone());

        self.grouped(|fmt| {
            write_chunk!(fmt, contract.loc.start(), "{}", contract.ty)?;
            write_chunk!(fmt, contract.name.loc.end(), "{}", contract.name.name)?;
            if !contract.base.is_empty() {
                write_chunk!(
                    fmt,
                    contract.name.loc.end(),
                    contract.base.first().unwrap().loc.start(),
                    "is"
                )?;
            }
            Ok(())
        })?;

        if !contract.base.is_empty() {
            self.indented(1, |fmt| {
                let base_end = contract.parts.first().map(|part| part.loc().start());
                let bases = fmt.items_to_chunks(
                    base_end,
                    contract.base.iter_mut().map(|base| Ok((base.loc, base))),
                )?;
                let multiline = fmt.are_chunks_separated_multiline("{}", &bases, ",")?;
                fmt.write_chunks_separated(&bases, ",", multiline)?;
                fmt.write_whitespace_separator(multiline)?;
                Ok(())
            })?;
        }

        write_chunk!(self, "{{")?;

        self.indented(1, |fmt| {
            if let Some(first) = contract.parts.first() {
                fmt.write_postfix_comments_before(first.loc().start())?;
                fmt.write_whitespace_separator(true)?;
            } else {
                return Ok(())
            }

            fmt.write_lined_visitable(
                contract.parts.iter_mut(),
                |last_part, part| match last_part {
                    Some(last_part) => match last_part {
                        ContractPart::ErrorDefinition(_) => {
                            !matches!(part, ContractPart::ErrorDefinition(_))
                        }
                        ContractPart::EventDefinition(_) => {
                            !matches!(part, ContractPart::EventDefinition(_))
                        }
                        ContractPart::VariableDefinition(_) => {
                            !matches!(part, ContractPart::VariableDefinition(_))
                        }
                        ContractPart::TypeDefinition(_) => {
                            !matches!(part, ContractPart::TypeDefinition(_))
                        }
                        ContractPart::EnumDefinition(_) => {
                            !matches!(part, ContractPart::EnumDefinition(_))
                        }
                        ContractPart::Using(_) => !matches!(part, ContractPart::Using(_)),
                        ContractPart::FunctionDefinition(last_def) => {
                            if last_def.is_empty() {
                                match part {
                                    ContractPart::FunctionDefinition(def) => !def.is_empty(),
                                    _ => true,
                                }
                            } else {
                                true
                            }
                        }
                        ContractPart::DocComment(_) => false,
                        _ => true,
                    },
                    None => false,
                },
            )
        })?;

        // the end of the contract location doesn't actually contain the }, this will change in
        // future versions of solang_parser
        let contract_end = {
            let last_byte = contract
                .parts
                .last()
                .map(|p| p.loc())
                .or_else(|| contract.base.last().map(|b| b.loc))
                .unwrap_or(contract.loc)
                .end();
            self.find_next_in_src(last_byte, '}').unwrap_or(last_byte)
        };
        self.write_postfix_comments_before(contract_end)?;
        if !contract.parts.is_empty() {
            self.write_whitespace_separator(true)?;
        }
        write_chunk!(self, contract_end, "}}")?;

        self.context.contract = None;

        Ok(())
    }

    fn visit_pragma(&mut self, ident: &mut Identifier, str: &mut StringLiteral) -> Result<()> {
        #[allow(clippy::if_same_then_else)]
        let pragma_descriptor = if ident.name == "solidity" {
            // There are some issues with parsing Solidity's versions with crates like `semver`:
            // 1. Ranges like `>=0.4.21<0.6.0` or `>=0.4.21 <0.6.0` are not parseable at all.
            // 2. Versions like `0.8.10` got transformed into `^0.8.10` which is not the same.
            // TODO: semver-solidity crate :D
            &str.string
        } else {
            &str.string
        };

        write_chunk!(self, str.loc.end(), "pragma {} {};", &ident.name, pragma_descriptor)?;

        Ok(())
    }

    fn visit_import_plain(&mut self, loc: Loc, import: &mut StringLiteral) -> Result<()> {
        self.grouped(|fmt| {
            write_chunk!(fmt, loc.start(), import.loc.start(), "import")?;
            write_chunk!(fmt, import.loc.start(), import.loc.end(), "\"{}\"", &import.string)?;
            fmt.write_semicolon()?;
            Ok(())
        })?;
        Ok(())
    }

    fn visit_import_global(
        &mut self,
        loc: Loc,
        global: &mut StringLiteral,
        alias: &mut Identifier,
    ) -> Result<()> {
        self.grouped(|fmt| {
            write_chunk!(fmt, loc.start(), global.loc.start(), "import")?;
            write_chunk!(fmt, global.loc.start(), global.loc.end(), "\"{}\"", &global.string)?;
            write_chunk!(fmt, loc.start(), alias.loc.start(), "as")?;
            alias.visit(fmt)?;
            fmt.write_semicolon()?;
            Ok(())
        })?;
        Ok(())
    }

    fn visit_import_renames(
        &mut self,
        loc: Loc,
        imports: &mut [(Identifier, Option<Identifier>)],
        from: &mut StringLiteral,
    ) -> Result<()> {
        if imports.is_empty() {
            self.grouped(|fmt| {
                write_chunk!(fmt, loc.start(), "import")?;
                fmt.write_empty_brackets()?;
                write_chunk!(fmt, loc.start(), from.loc.start(), "from")?;
                write_chunk!(fmt, from.loc.start(), from.loc.end(), "\"{}\"", &from.string)?;
                fmt.write_semicolon()?;
                Ok(())
            })?;
            return Ok(())
        }

        let imports_start = imports.first().unwrap().0.loc.start();

        write_chunk!(self, loc.start(), imports_start, "import")?;

        self.surrounded(imports_start, "{", "}", Some(from.loc.start()), |fmt, _multiline| {
            let mut imports = imports.iter_mut().peekable();
            let mut import_chunks = Vec::new();
            while let Some((ident, alias)) = imports.next() {
                import_chunks.push(fmt.chunked(
                    ident.loc.start(),
                    imports.peek().map(|(ident, _)| ident.loc.start()),
                    |fmt| {
                        fmt.grouped(|fmt| {
                            ident.visit(fmt)?;
                            if let Some(alias) = alias {
                                write_chunk!(fmt, ident.loc.end(), alias.loc.start(), "as")?;
                                alias.visit(fmt)?;
                            }
                            Ok(())
                        })?;
                        Ok(())
                    },
                )?);
            }

            let multiline = fmt.are_chunks_separated_multiline(
                &format!("{{}} }} from \"{}\";", from.string),
                &import_chunks,
                ",",
            )?;
            fmt.write_chunks_separated(&import_chunks, ",", multiline)?;
            Ok(())
        })?;

        self.grouped(|fmt| {
            write_chunk!(fmt, imports_start, from.loc.start(), "from")?;
            write_chunk!(fmt, from.loc.start(), from.loc.end(), "\"{}\"", &from.string)?;
            fmt.write_semicolon()?;
            Ok(())
        })?;

        Ok(())
    }

    fn visit_enum(&mut self, enumeration: &mut EnumDefinition) -> Result<()> {
        let mut name = self.visit_to_chunk(
            enumeration.name.loc.start(),
            Some(enumeration.loc.end()),
            &mut enumeration.name,
        )?;
        name.content = format!("enum {}", name.content);
        self.write_chunk(&name)?;

        if enumeration.values.is_empty() {
            self.write_empty_brackets()?;
        } else {
            self.surrounded(
                enumeration.values.first().unwrap().loc.start(),
                "{",
                "}",
                Some(enumeration.loc.end()),
                |fmt, _multiline| {
                    let values = fmt.items_to_chunks(
                        Some(enumeration.loc.end()),
                        enumeration.values.iter_mut().map(|ident| Ok((ident.loc, ident))),
                    )?;
                    fmt.write_chunks_separated(&values, ",", true)?;
                    Ok(())
                },
            )?;
        }

        Ok(())
    }

    fn visit_expr(&mut self, loc: Loc, expr: &mut Expression) -> Result<()> {
        match expr {
            Expression::Type(loc, typ) => match typ {
                Type::Address => write_chunk!(self, loc.start(), "address")?,
                Type::AddressPayable => write_chunk!(self, loc.start(), "address payable")?,
                Type::Payable => write_chunk!(self, loc.start(), "payable")?,
                Type::Bool => write_chunk!(self, loc.start(), "bool")?,
                Type::String => write_chunk!(self, loc.start(), "string")?,
                Type::Int(n) => write_chunk!(self, loc.start(), "int{}", n)?,
                Type::Uint(n) => write_chunk!(self, loc.start(), "uint{}", n)?,
                Type::Bytes(n) => write_chunk!(self, loc.start(), "bytes{}", n)?,
                Type::Rational => write_chunk!(self, loc.start(), "rational")?,
                Type::DynamicBytes => write_chunk!(self, loc.start(), "bytes")?,
                Type::Mapping(loc, from, to) => {
                    write_chunk!(self, loc.start(), "mapping(")?;
                    from.visit(self)?;
                    write!(self.buf(), " => ")?;
                    to.visit(self)?;
                    write!(self.buf(), ")")?;
                }
                Type::Function { .. } => self.visit_source(*loc)?,
            },
            Expression::ArraySubscript(_, ty_exp, size_exp) => {
                ty_exp.visit(self)?;
                write!(self.buf(), "[")?;
                if let Some(size_exp) = size_exp {
                    size_exp.visit(self)?;
                }
                write!(self.buf(), "]")?;
            }
            Expression::PreIncrement(..) |
            Expression::PostIncrement(..) |
            Expression::PreDecrement(..) |
            Expression::PostDecrement(..) |
            Expression::Not(..) |
            Expression::Complement(..) |
            Expression::UnaryPlus(..) |
            Expression::Add(..) |
            Expression::UnaryMinus(..) |
            Expression::Subtract(..) |
            Expression::Power(..) |
            Expression::Multiply(..) |
            Expression::Divide(..) |
            Expression::Modulo(..) |
            Expression::ShiftLeft(..) |
            Expression::ShiftRight(..) |
            Expression::BitwiseAnd(..) |
            Expression::BitwiseXor(..) |
            Expression::BitwiseOr(..) |
            Expression::Less(..) |
            Expression::More(..) |
            Expression::LessEqual(..) |
            Expression::MoreEqual(..) |
            Expression::And(..) |
            Expression::Or(..) |
            Expression::Equal(..) |
            Expression::NotEqual(..) => {
                self.visit_flattened_expr(&mut expr.flatten())?;
            }
            Expression::Assign(..) |
            Expression::AssignOr(..) |
            Expression::AssignAnd(..) |
            Expression::AssignXor(..) |
            Expression::AssignShiftLeft(..) |
            Expression::AssignShiftRight(..) |
            Expression::AssignAdd(..) |
            Expression::AssignSubtract(..) |
            Expression::AssignMultiply(..) |
            Expression::AssignDivide(..) |
            Expression::AssignModulo(..) => {
                let precedence = expr.precedence();
                let op = expr.operator().unwrap();
                let (left, right) = expr.into_components();
                let left = left.unwrap();
                let right = right.unwrap();

                // we shouldn't need parentheses on the left as assignment should always have the
                // lowest priority
                let left_chunked = self.chunked(left.loc().start(), None, |fmt| left.visit(fmt))?;
                // TODO we use the operator presence as a short hand for whether and indent was
                // introduced if multiline, but this may not always be the case
                let left_is_indented =
                    left_chunked.content.contains('\n') && left.operator().is_some();
                self.write_chunk(&left_chunked)?;

                self.indented_if(left_is_indented || !self.will_it_fit(op), 1, |fmt| {
                    write_chunk!(fmt, "{}", op)?;
                    if precedence.is_evaluated_first(right.precedence()) {
                        fmt.surrounded(right.loc().start(), "(", ")", None, |fmt, _multiline| {
                            right.visit(fmt)
                        })?;
                    } else {
                        fmt.visit_assignment(right)?;
                    }
                    Ok(())
                })?;
            }
            Expression::Variable(ident) => {
                ident.visit(self)?;
            }
            Expression::MemberAccess(_, expr, ident) => {
                let (remaining, idents) = {
                    let mut idents = vec![ident];
                    let mut remaining = expr.as_mut();
                    while let Expression::MemberAccess(_, expr, ident) = remaining {
                        idents.push(ident);
                        remaining = expr;
                    }
                    idents.reverse();
                    (remaining, idents)
                };

                self.visit_expr(remaining.loc(), remaining)?;

                let mut chunks = self.items_to_chunks(
                    Some(loc.end()),
                    idents.into_iter().map(|ident| Ok((ident.loc, ident))),
                )?;
                chunks.iter_mut().for_each(|chunk| chunk.content.insert(0, '.'));
                let multiline = self.are_chunks_separated_multiline("{}", &chunks, "")?;
                self.write_chunks_separated(&chunks, "", multiline)?;
            }
            _ => self.visit_source(loc)?,
        };

        Ok(())
    }

    fn visit_ident(&mut self, loc: Loc, ident: &mut Identifier) -> Result<()> {
        write_chunk!(self, loc.end(), "{}", ident.name)?;
        Ok(())
    }

    fn visit_emit(&mut self, loc: Loc, event: &mut Expression) -> Result<()> {
        self.grouped(|fmt| {
            write_chunk!(fmt, loc.start(), "emit")?;
            event.visit(fmt)?;
            fmt.write_semicolon()?;
            Ok(())
        })?;
        Ok(())
    }

    fn visit_var_declaration(
        &mut self,
        var: &mut VariableDeclaration,
        is_assignment: bool,
    ) -> Result<()> {
        self.grouped(|fmt| {
            var.ty.visit(fmt)?;
            if let Some(storage) = &var.storage {
                write_chunk!(fmt, storage.loc().end(), "{}", storage)?;
            }
            write_chunk!(
                fmt,
                var.name.loc.end(),
                "{}{}",
                var.name.name,
                if is_assignment { " =" } else { "" }
            )?;
            Ok(())
        })?;
        Ok(())
    }

    fn visit_break(&mut self) -> Result<()> {
        write_chunk!(self, "break;")?;
        Ok(())
    }

    fn visit_continue(&mut self) -> Result<()> {
        write_chunk!(self, "continue;")?;
        Ok(())
    }

    fn visit_function(&mut self, func: &mut FunctionDefinition) -> Result<()> {
        self.context.function = Some(func.clone());

        self.write_postfix_comments_before(func.loc.start())?;
        self.write_prefix_comments_before(func.loc.start())?;

        let func_name = if let Some(ident) = &func.name {
            format!("{} {}", func.ty, ident.name)
        } else {
            func.ty.to_string()
        };

        // calculate locations of chunk groups
        let attrs_loc = func.attributes.first().map(|attr| attr.loc());
        let returns_loc = func.returns.first().map(|param| param.0);
        let body_loc = func.body.as_ref().map(LineOfCode::loc);

        let params_end = attrs_loc
            .as_ref()
            .or(returns_loc.as_ref())
            .or(body_loc.as_ref())
            .map(|loc| loc.start());
        let attrs_end = returns_loc.as_ref().or(body_loc.as_ref()).map(|loc| loc.start());
        let returns_end = body_loc.as_ref().map(|loc| loc.start());

        let mut params_multiline = false;
        self.surrounded(
            func.loc.start(),
            format!("{func_name}("),
            ")",
            params_end,
            |fmt, multiline| {
                let params = fmt.items_to_chunks(
                    params_end,
                    func.params.iter_mut().map(|(loc, param)| Ok((*loc, param.as_mut().unwrap()))),
                )?;
                let after_params = if !func.attributes.is_empty() || !func.returns.is_empty() {
                    ""
                } else if func.body.is_some() {
                    " {"
                } else {
                    ";"
                };
                params_multiline = multiline ||
                    fmt.are_chunks_separated_multiline(
                        &format!("{{}}){after_params}"),
                        &params,
                        ",",
                    )?;
                fmt.write_chunks_separated(&params, ",", params_multiline)?;
                Ok(())
            },
        )?;

        let mut write_attributes = |fmt: &mut Self, multiline: bool| -> Result<()> {
            // write attributes
            if !func.attributes.is_empty() {
                let byte_offset = attrs_loc.unwrap().start();
                fmt.write_postfix_comments_before(byte_offset)?;
                fmt.write_whitespace_separator(multiline)?;
                let attributes = fmt.items_to_chunks_sorted(
                    attrs_end,
                    func.attributes.iter_mut().map(|attr| Ok((attr.loc(), attr))),
                )?;
                fmt.indented(1, |fmt| {
                    fmt.write_chunks_separated(&attributes, "", multiline)?;
                    Ok(())
                })?;
            }

            // write returns
            if !func.returns.is_empty() {
                let returns = fmt.items_to_chunks(
                    returns_end,
                    func.returns.iter_mut().map(|(loc, param)| Ok((*loc, param.as_mut().unwrap()))),
                )?;
                let byte_offset = returns_loc.unwrap().start();
                fmt.write_postfix_comments_before(byte_offset)?;
                fmt.write_whitespace_separator(multiline)?;
                fmt.indented(1, |fmt| {
                    fmt.surrounded(
                        byte_offset,
                        "returns (",
                        ")",
                        returns_end,
                        |fmt, multiline_hint| {
                            fmt.write_chunks_separated(&returns, ",", multiline_hint)?;
                            Ok(())
                        },
                    )?;
                    Ok(())
                })?;
            }
            Ok(())
        };

        let attrs_multiline = params_multiline ||
            !self.try_on_single_line(|fmt| {
                write_attributes(fmt, false)?;
                if !fmt.will_it_fit(if func.body.is_some() { " {" } else { ";" }) {
                    bail!(FormatterError::fmt())
                }
                Ok(())
            })?;
        if attrs_multiline {
            write_attributes(self, true)?;
        }

        // write function body
        match &mut func.body {
            Some(body) => {
                let body_loc = body_loc.unwrap();
                let byte_offset = body_loc.start();
                let body = self.visit_to_chunk(byte_offset, Some(body_loc.end()), body)?;
                self.write_whitespace_separator(
                    attrs_multiline && !(func.attributes.is_empty() && func.returns.is_empty()),
                )?;
                self.write_chunk(&body)?;
            }
            None => self.write_semicolon()?,
        }

        self.context.function = None;

        Ok(())
    }

    fn visit_function_attribute(&mut self, attribute: &mut FunctionAttribute) -> Result<()> {
        match attribute {
            FunctionAttribute::Mutability(mutability) => {
                write_chunk!(self, mutability.loc().end(), "{mutability}")?
            }
            FunctionAttribute::Visibility(visibility) => {
                // Visibility will always have a location in a Function attribute
                write_chunk!(self, visibility.loc().unwrap().end(), "{visibility}")?
            }
            FunctionAttribute::Virtual(loc) => write_chunk!(self, loc.end(), "virtual")?,
            FunctionAttribute::Immutable(loc) => write_chunk!(self, loc.end(), "immutable")?,
            FunctionAttribute::Override(loc, args) => {
                write_chunk!(self, loc.start(), "override")?;
                if !args.is_empty() {
                    self.surrounded(loc.start(), "(", ")", Some(loc.end()), |fmt, _multiline| {
                        let args = fmt.items_to_chunks(
                            Some(loc.end()),
                            args.iter_mut().map(|arg| Ok((arg.loc, arg))),
                        )?;
                        let multiline = fmt.are_chunks_separated_multiline("{}", &args, ", ")?;
                        fmt.write_chunks_separated(&args, ",", multiline)?;
                        Ok(())
                    })?;
                }
            }
            FunctionAttribute::BaseOrModifier(loc, base) => {
                let is_contract_base = self.context.contract.as_ref().map_or(false, |contract| {
                    contract.base.iter().any(|contract_base| {
                        helpers::namespace_matches(&contract_base.name, &base.name)
                    })
                });

                if is_contract_base {
                    base.visit(self)?;
                } else {
                    let mut base_or_modifier =
                        self.visit_to_chunk(loc.start(), Some(loc.end()), base)?;
                    if base_or_modifier.content.ends_with("()") {
                        base_or_modifier.content.truncate(base_or_modifier.content.len() - 2);
                    }
                    self.write_chunk(&base_or_modifier)?;
                }
            }
        };

        Ok(())
    }

    fn visit_base(&mut self, base: &mut Base) -> Result<()> {
        let name_loc = LineOfCode::loc(&base.name);
        let mut name = self.chunked(name_loc.start(), Some(name_loc.end()), |fmt| {
            fmt.grouped(|fmt| {
                fmt.visit_expr(LineOfCode::loc(&base.name), &mut base.name)?;
                Ok(())
            })?;
            Ok(())
        })?;

        if base.args.is_none() || base.args.as_ref().unwrap().is_empty() {
            if self.context.function.is_some() {
                name.content.push_str("()");
            }
            self.write_chunk(&name)?;
            return Ok(())
        }

        let args = base.args.as_mut().unwrap();
        let args_start = LineOfCode::loc(args.first().unwrap()).start();

        name.content.push('(');
        let formatted_name = self.chunk_to_string(&name)?;

        let multiline = !self.will_it_fit(&formatted_name);

        self.surrounded(
            args_start,
            &formatted_name,
            ")",
            Some(base.loc.end()),
            |fmt, multiline_hint| {
                let args = fmt.items_to_chunks(
                    Some(base.loc.end()),
                    args.iter_mut().map(|arg| Ok((arg.loc(), arg))),
                )?;
                let multiline = multiline ||
                    multiline_hint ||
                    fmt.are_chunks_separated_multiline("{}", &args, ",")?;
                fmt.write_chunks_separated(&args, ",", multiline)?;
                Ok(())
            },
        )?;

        Ok(())
    }

    fn visit_parameter(&mut self, parameter: &mut Parameter) -> Result<()> {
        self.grouped(|fmt| {
            parameter.ty.visit(fmt)?;
            if let Some(storage) = &parameter.storage {
                write_chunk!(fmt, storage.loc().end(), "{storage}")?;
            }
            if let Some(name) = &parameter.name {
                write_chunk!(fmt, parameter.loc.end(), "{}", name.name)?;
            }
            Ok(())
        })?;
        Ok(())
    }

    fn visit_struct(&mut self, structure: &mut StructDefinition) -> Result<()> {
        let mut name = self.visit_to_chunk(
            structure.name.loc.start(),
            Some(structure.loc.end()),
            &mut structure.name,
        )?;
        name.content = format!("struct {}", name.content);
        self.write_chunk(&name)?;

        if structure.fields.is_empty() {
            self.write_empty_brackets()?;
        } else {
            self.surrounded(
                structure.fields.first().unwrap().loc.start(),
                "{",
                "}",
                Some(structure.loc.end()),
                |fmt, _multiline| {
                    let chunks = fmt.items_to_chunks(
                        Some(structure.loc.end()),
                        structure.fields.iter_mut().map(|ident| Ok((ident.loc, ident))),
                    )?;
                    for mut chunk in chunks {
                        chunk.content.push(';');
                        fmt.write_chunk(&chunk)?;
                        fmt.write_whitespace_separator(true)?;
                    }
                    Ok(())
                },
            )?;
        }

        Ok(())
    }

    fn visit_type_definition(&mut self, def: &mut TypeDefinition) -> Result<()> {
        self.grouped(|fmt| {
            write_chunk!(fmt, def.loc.start(), def.name.loc.start(), "type")?;
            def.name.visit(fmt)?;
            write_chunk!(fmt, def.name.loc.end(), LineOfCode::loc(&def.ty).start(), "is")?;
            def.ty.visit(fmt)?;
            fmt.write_semicolon()?;
            Ok(())
        })?;
        Ok(())
    }

    fn visit_stray_semicolon(&mut self) -> Result<()> {
        self.write_semicolon()?;
        Ok(())
    }

    fn visit_block(
        &mut self,
        loc: Loc,
        unchecked: bool,
        statements: &mut Vec<Statement>,
    ) -> Result<()> {
        if unchecked {
            write_chunk!(self, loc.start(), "unchecked ")?;
        }
        write_chunk!(self, "{{")?;

        self.indented(1, |fmt| {
            if let Some(statement) = statements.first() {
                fmt.write_postfix_comments_before(LineOfCode::loc(statement).start())?;
                fmt.write_whitespace_separator(true)?;
            } else {
                return Ok(())
            }

            fmt.write_lined_visitable(statements.iter_mut(), |_, _| false)?;
            Ok(())
        })?;

        self.write_postfix_comments_before(loc.end())?;
        if !statements.is_empty() {
            self.write_whitespace_separator(true)?;
        }
        write_chunk!(self, loc.end(), "}}")?;

        Ok(())
    }

    fn visit_opening_paren(&mut self) -> Result<()> {
        write_chunk!(self, "(")?;
        Ok(())
    }

    fn visit_closing_paren(&mut self) -> Result<()> {
        write_chunk!(self, ")")?;
        Ok(())
    }

    fn visit_newline(&mut self) -> Result<()> {
        writeln_chunk!(self)?;
        Ok(())
    }

    fn visit_event(&mut self, event: &mut EventDefinition) -> Result<()> {
        let mut name =
            self.visit_to_chunk(event.name.loc.start(), Some(event.loc.end()), &mut event.name)?;
        name.content = format!("event {}(", name.content);

        if event.fields.is_empty() {
            name.content.push(')');
            self.write_chunk(&name)?;
        } else {
            let params_start = event.fields.first().unwrap().loc.start();

            let formatted_name = self.chunk_to_string(&name)?;

            self.surrounded(params_start, &formatted_name, ")", None, |fmt, multiline| {
                let params = fmt
                    .items_to_chunks(None, event.fields.iter_mut().map(|arg| Ok((arg.loc, arg))))?;
                fmt.write_chunks_separated(&params, ",", multiline)?;
                Ok(())
            })?;
        }

        self.grouped(|fmt| {
            write_chunk!(
                fmt,
                event.loc.start(),
                event.loc.end(),
                "{}",
                if event.anonymous { "anonymous" } else { "" }
            )
        })?;

        self.write_semicolon()?;

        Ok(())
    }

    fn visit_event_parameter(&mut self, param: &mut EventParameter) -> Result<()> {
        self.grouped(|fmt| {
            param.ty.visit(fmt)?;
            if param.indexed {
                write_chunk!(fmt, param.loc.start(), "indexed")?;
            }
            if let Some(name) = &param.name {
                write_chunk!(fmt, name.loc.end(), "{}", name.name)?;
            }
            Ok(())
        })?;
        Ok(())
    }

    fn visit_error(&mut self, error: &mut ErrorDefinition) -> Result<()> {
        let mut name =
            self.visit_to_chunk(error.name.loc.start(), Some(error.loc.end()), &mut error.name)?;
        name.content = format!("error {}(", name.content);

        if error.fields.is_empty() {
            name.content.push(')');
            self.write_chunk(&name)?;
        } else {
            let params_start = error.fields.first().unwrap().loc.start();

            let formatted_name = self.chunk_to_string(&name)?;

            self.surrounded(params_start, &formatted_name, ")", None, |fmt, _multiline| {
                let params = fmt
                    .items_to_chunks(None, error.fields.iter_mut().map(|arg| Ok((arg.loc, arg))))?;
                let multiline = fmt.are_chunks_separated_multiline("{}", &params, ",")?;
                fmt.write_chunks_separated(&params, ",", multiline)?;
                Ok(())
            })?;
        }

        self.write_semicolon()?;

        Ok(())
    }

    fn visit_error_parameter(&mut self, param: &mut ErrorParameter) -> Result<()> {
        self.grouped(|fmt| {
            param.ty.visit(fmt)?;
            if let Some(name) = &param.name {
                write_chunk!(fmt, name.loc.end(), "{}", name.name)?;
            }
            Ok(())
        })?;
        Ok(())
    }

    fn visit_using(&mut self, using: &mut Using) -> Result<()> {
        write_chunk!(self, using.loc.start(), "using")?;

        let ty_start = using.ty.as_mut().map(|ty| LineOfCode::loc(&ty).start());
        let global_start = using.global.as_mut().map(|global| global.loc.start());
        let loc_end = using.loc.end();

        let (is_library, mut list_chunks) = match &mut using.list {
            UsingList::Library(library) => {
                (true, vec![self.visit_to_chunk(library.loc().start(), None, library)?])
            }
            UsingList::Functions(funcs) => {
                let mut funcs = funcs.iter_mut().peekable();
                let mut chunks = Vec::new();
                while let Some(func) = funcs.next() {
                    let next_byte_end = funcs.peek().map(|func| func.loc().start());
                    chunks.push(self.chunked(func.loc().start(), next_byte_end, |fmt| {
                        fmt.grouped(|fmt| fmt.visit_expr(func.loc(), func))?;
                        Ok(())
                    })?);
                }
                (false, chunks)
            }
        };

        let for_chunk = self.chunk_at(
            using.loc.start(),
            Some(ty_start.or(global_start).unwrap_or(loc_end)),
            "for",
        );
        let ty_chunk = if let Some(ty) = &mut using.ty {
            self.visit_to_chunk(ty.loc().start(), Some(global_start.unwrap_or(loc_end)), ty)?
        } else {
            self.chunk_at(using.loc.start(), Some(global_start.unwrap_or(loc_end)), "*")
        };
        let global_chunk = using
            .global
            .as_mut()
            .map(|global| self.visit_to_chunk(global.loc.start(), Some(using.loc.end()), global))
            .transpose()?;

        let write_for_def = |fmt: &mut Self| {
            fmt.grouped(|fmt| {
                fmt.write_chunk(&for_chunk)?;
                fmt.write_chunk(&ty_chunk)?;
                if let Some(global_chunk) = global_chunk.as_ref() {
                    fmt.write_chunk(global_chunk)?;
                }
                Ok(())
            })?;
            Ok(())
        };

        let simulated_for_def = self.simulate_to_string(write_for_def)?;

        if is_library {
            let chunk = list_chunks.pop().unwrap();
            if self.will_chunk_fit(&format!("{{}} {simulated_for_def};"), &chunk)? {
                self.write_chunk(&chunk)?;
                write_for_def(self)?;
            } else {
                self.write_whitespace_separator(true)?;
                self.grouped(|fmt| {
                    fmt.write_chunk(&chunk)?;
                    Ok(())
                })?;
                self.write_whitespace_separator(true)?;
                write_for_def(self)?;
            }
        } else {
            self.surrounded(
                using.loc.start(),
                "{",
                "}",
                Some(ty_start.or(global_start).unwrap_or(loc_end)),
                |fmt, _multiline| {
                    let multiline = fmt.are_chunks_separated_multiline(
                        &format!("{{ {{}} }} {simulated_for_def};"),
                        &list_chunks,
                        ",",
                    )?;
                    fmt.write_chunks_separated(&list_chunks, ",", multiline)?;
                    Ok(())
                },
            )?;
            write_for_def(self)?;
        }

        self.write_semicolon()?;

        Ok(())
    }

    fn visit_var_attribute(&mut self, attribute: &mut VariableAttribute) -> Result<()> {
        let token = match attribute {
            VariableAttribute::Visibility(visibility) => visibility.to_string(),
            VariableAttribute::Constant(_) => "constant".to_string(),
            VariableAttribute::Immutable(_) => "immutable".to_string(),
            VariableAttribute::Override(_) => "override".to_string(),
        };
        let loc = attribute.loc();
        write_chunk!(self, loc.start(), loc.end(), "{}", token)?;
        Ok(())
    }

    fn visit_var_definition(&mut self, var: &mut VariableDefinition) -> Result<()> {
        var.ty.visit(self)?;

        let multiline = self.grouped(|fmt| {
            let name_start = var.name.loc.start();

            let attrs = fmt.items_to_chunks_sorted(
                Some(name_start),
                var.attrs.iter_mut().map(|attr| Ok((attr.loc(), attr))),
            )?;
            if !fmt.try_on_single_line(|fmt| fmt.write_chunks_separated(&attrs, "", false))? {
                fmt.write_chunks_separated(&attrs, "", true)?;
            }

            let mut name =
                fmt.visit_to_chunk(name_start, Some(var.name.loc.end()), &mut var.name)?;
            if var.initializer.is_some() {
                name.content.push_str(" =");
            }
            fmt.write_chunk(&name)?;

            Ok(())
        })?;

        var.initializer
            .as_mut()
            .map(|init| self.indented_if(multiline, 1, |fmt| fmt.visit_assignment(init)))
            .transpose()?;

        self.write_semicolon()?;

        Ok(())
    }

    fn visit_var_definition_stmt(
        &mut self,
        _loc: Loc,
        declaration: &mut VariableDeclaration,
        expr: &mut Option<Expression>,
        semicolon: bool,
    ) -> Result<()> {
        let declaration = self.chunked(declaration.loc.start(), None, |fmt| {
            fmt.visit_var_declaration(declaration, expr.is_some())
        })?;
        let multiline = declaration.content.contains('\n');
        self.write_chunk(&declaration)?;

        expr.as_mut()
            .map(|expr| self.indented_if(multiline, 1, |fmt| fmt.visit_assignment(expr)))
            .transpose()?;

        if semicolon {
            self.write_semicolon()?;
        }
        Ok(())
    }

    fn visit_for(
        &mut self,
        loc: Loc,
        init: &mut Option<Box<Statement>>,
        cond: &mut Option<Box<Expression>>,
        update: &mut Option<Box<Statement>>,
        body: &mut Option<Box<Statement>>,
    ) -> Result<(), Self::Error> {
        let next_byte_end = update.as_ref().map(|u| u.loc().end());
        self.surrounded(loc.start(), "for (", ") ", next_byte_end, |fmt, _| {
            let mut write_for_loop_header = |fmt: &mut Self, multiline: bool| -> Result<()> {
                init.as_mut()
                    .map(|stmt| {
                        match **stmt {
                            Statement::VariableDefinition(loc, ref mut decl, ref mut expr) => {
                                fmt.visit_var_definition_stmt(loc, decl, expr, false)
                            }
                            Statement::Expression(loc, ref mut expr) => fmt.visit_expr(loc, expr),
                            _ => stmt.visit(fmt), // unreachable
                        }
                    })
                    .transpose()?;
                fmt.write_semicolon()?;
                if multiline {
                    fmt.write_whitespace_separator(true)?;
                }
                cond.as_mut().map(|expr| expr.visit(fmt)).transpose()?;
                fmt.write_semicolon()?;
                if multiline {
                    fmt.write_whitespace_separator(true)?;
                }
                update
                    .as_mut()
                    .map(|stmt| {
                        match **stmt {
                            Statement::VariableDefinition(_, ref mut decl, ref mut expr) => {
                                fmt.visit_var_definition_stmt(loc, decl, expr, false)
                            }
                            Statement::Expression(loc, ref mut expr) => fmt.visit_expr(loc, expr),
                            _ => stmt.visit(fmt), // unreachable
                        }
                    })
                    .transpose()?;
                Ok(())
            };
            let multiline = !fmt.try_on_single_line(|fmt| write_for_loop_header(fmt, false))?;
            if multiline {
                write_for_loop_header(fmt, true)?;
            }
            Ok(())
        })?;
        match body {
            Some(body) => body.visit(self),
            None => self.write_empty_brackets(),
        }
    }

    fn visit_while(
        &mut self,
        loc: Loc,
        cond: &mut Expression,
        body: &mut Statement,
    ) -> Result<(), Self::Error> {
        self.surrounded(loc.start(), "while (", ") ", Some(cond.loc().end()), |fmt, _| {
            cond.visit(fmt)
        })?;
        body.visit(self)
    }

    fn visit_do_while(
        &mut self,
        loc: Loc,
        body: &mut Statement,
        cond: &mut Expression,
    ) -> Result<(), Self::Error> {
        write_chunk!(self, loc.start(), "do ")?;
        body.visit(self)?;
        self.surrounded(body.loc().end(), "while (", ");", Some(cond.loc().end()), |fmt, _| {
            cond.visit(fmt)
        })
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;
    use std::{fs, path::PathBuf};

    use crate::visit::Visitable;

    use super::*;

    fn test_directory(base_name: &str) {
        let mut original = None;

        let tests = fs::read_dir(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata").join(base_name),
        )
        .unwrap()
        .filter_map(|path| {
            let path = path.unwrap().path();
            let source = fs::read_to_string(&path).unwrap();

            if let Some(filename) = path.file_name().and_then(|name| name.to_str()) {
                if filename == "original.sol" {
                    original = Some(source);
                } else if filename
                    .strip_suffix("fmt.sol")
                    .map(|filename| filename.strip_suffix('.'))
                    .is_some()
                {
                    let mut config = FormatterConfig::default();

                    let mut lines = source.split('\n').peekable();
                    while let Some(line) = lines.peek() {
                        let entry = line
                            .strip_prefix("//")
                            .and_then(|line| line.trim().strip_prefix("config:"))
                            .map(str::trim);
                        if entry.is_none() {
                            break
                        }

                        if let Some((key, value)) = entry.unwrap().split_once('=') {
                            match key {
                                "line-length" => config.line_length = value.parse().unwrap(),
                                "tab-width" => config.tab_width = value.parse().unwrap(),
                                "bracket-spacing" => {
                                    config.bracket_spacing = value.parse().unwrap()
                                }
                                _ => panic!("Unknown config key: {key}"),
                            }
                        }

                        lines.next();
                    }

                    return Some((filename.to_string(), config, lines.join("\n")))
                }
            }

            None
        })
        .collect::<Vec<_>>();

        for (filename, config, formatted) in tests {
            test_formatter(
                &filename,
                config,
                original.as_ref().expect("original.sol not found"),
                &formatted,
            );
        }
    }

    fn test_formatter(
        filename: &str,
        config: FormatterConfig,
        source: &str,
        expected_source: &str,
    ) {
        #[derive(PartialEq, Eq)]
        struct PrettyString(String);

        impl std::fmt::Debug for PrettyString {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        let (mut source_pt, source_comments) = solang_parser::parse(source, 1).unwrap();
        let source_comments = Comments::new(source_comments, source);

        let (mut expected_pt, expected_comments) =
            solang_parser::parse(expected_source, 1).unwrap();
        if !source_pt.ast_eq(&expected_pt) {
            pretty_assertions::assert_eq!(
                source_pt,
                expected_pt,
                "(formatted Parse Tree == expected Parse Tree) in {}",
                filename
            );
        }
        let expected_comments = Comments::new(expected_comments, expected_source);

        let expected = PrettyString(expected_source.trim().to_string());

        let mut source_formatted = String::new();
        let mut f = Formatter::new(&mut source_formatted, source, source_comments, config.clone());
        source_pt.visit(&mut f).unwrap();

        println!("{}", source_formatted);
        let source_formatted = PrettyString(source_formatted);

        pretty_assertions::assert_eq!(
            source_formatted,
            expected,
            "(formatted == expected) in {}",
            filename
        );

        let mut expected_formatted = String::new();
        let mut f =
            Formatter::new(&mut expected_formatted, expected_source, expected_comments, config);
        expected_pt.visit(&mut f).unwrap();
        let expected_formatted = PrettyString(expected_formatted);

        pretty_assertions::assert_eq!(
            expected_formatted,
            expected,
            "(formatted == expected) in {}",
            filename
        );
    }

    macro_rules! test_directory {
        ($dir:ident) => {
            #[allow(non_snake_case)]
            #[test]
            fn $dir() {
                test_directory(stringify!($dir));
            }
        };
    }

    test_directory! { ConstructorDefinition }
    test_directory! { ContractDefinition }
    test_directory! { DocComments }
    test_directory! { EnumDefinition }
    test_directory! { ErrorDefinition }
    test_directory! { EventDefinition }
    test_directory! { FunctionDefinition }
    test_directory! { FunctionType }
    test_directory! { ImportDirective }
    test_directory! { ModifierDefinition }
    test_directory! { StatementBlock }
    test_directory! { StructDefinition }
    test_directory! { TypeDefinition }
    test_directory! { UsingDirective }
    test_directory! { VariableDefinition }
    test_directory! { ExpressionPrecedence }
    test_directory! { WhileStatement }
    test_directory! { DoWhileStatement }
    test_directory! { ForStatement }
    test_directory! { IfStatement }
}
