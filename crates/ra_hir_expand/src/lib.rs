//! `ra_hir_expand` deals with macro expansion.
//!
//! Specifically, it implements a concept of `MacroFile` -- a file whose syntax
//! tree originates not from the text of some `FileId`, but from some macro
//! expansion.

pub mod db;
pub mod ast_id_map;
pub mod either;
pub mod name;
pub mod hygiene;
pub mod diagnostics;

use std::hash::{Hash, Hasher};

use ra_db::{salsa, CrateId, FileId};
use ra_syntax::{
    ast::{self, AstNode},
    SyntaxNode, TextRange,
};

use crate::ast_id_map::FileAstId;
use std::sync::Arc;

/// Input to the analyzer is a set of files, where each file is identified by
/// `FileId` and contains source code. However, another source of source code in
/// Rust are macros: each macro can be thought of as producing a "temporary
/// file". To assign an id to such a file, we use the id of the macro call that
/// produced the file. So, a `HirFileId` is either a `FileId` (source code
/// written by user), or a `MacroCallId` (source code produced by macro).
///
/// What is a `MacroCallId`? Simplifying, it's a `HirFileId` of a file
/// containing the call plus the offset of the macro call in the file. Note that
/// this is a recursive definition! However, the size_of of `HirFileId` is
/// finite (because everything bottoms out at the real `FileId`) and small
/// (`MacroCallId` uses the location interner).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HirFileId(HirFileIdRepr);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum HirFileIdRepr {
    FileId(FileId),
    MacroFile(MacroFile),
}

impl From<FileId> for HirFileId {
    fn from(id: FileId) -> Self {
        HirFileId(HirFileIdRepr::FileId(id))
    }
}

impl From<MacroFile> for HirFileId {
    fn from(id: MacroFile) -> Self {
        HirFileId(HirFileIdRepr::MacroFile(id))
    }
}

impl HirFileId {
    /// For macro-expansion files, returns the file original source file the
    /// expansion originated from.
    pub fn original_file(self, db: &dyn db::AstDatabase) -> FileId {
        match self.0 {
            HirFileIdRepr::FileId(file_id) => file_id,
            HirFileIdRepr::MacroFile(macro_file) => {
                let loc = db.lookup_intern_macro(macro_file.macro_call_id);
                loc.ast_id.file_id().original_file(db)
            }
        }
    }

    /// Return expansion information if it is a macro-expansion file
    pub fn parent_expansion(
        self,
        db: &dyn db::AstDatabase,
    ) -> Option<((HirFileId, HirFileId), Arc<ExpansionInfo>)> {
        match self.0 {
            HirFileIdRepr::FileId(_) => None,
            HirFileIdRepr::MacroFile(macro_file) => {
                let loc: MacroCallLoc = db.lookup_intern_macro(macro_file.macro_call_id);

                let def_file = loc.def.ast_id.file_id;
                let arg_file = loc.ast_id.file_id;

                db.macro_expansion_info(macro_file).map(|ex| ((arg_file, def_file), ex))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacroFile {
    macro_call_id: MacroCallId,
    macro_file_kind: MacroFileKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacroFileKind {
    Items,
    Expr,
}

/// `MacroCallId` identifies a particular macro invocation, like
/// `println!("Hello, {}", world)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacroCallId(salsa::InternId);
impl salsa::InternKey for MacroCallId {
    fn from_intern_id(v: salsa::InternId) -> Self {
        MacroCallId(v)
    }
    fn as_intern_id(&self) -> salsa::InternId {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacroDefId {
    pub krate: CrateId,
    pub ast_id: AstId<ast::MacroCall>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MacroCallLoc {
    pub def: MacroDefId,
    pub ast_id: AstId<ast::MacroCall>,
}

impl MacroCallId {
    pub fn as_file(self, kind: MacroFileKind) -> HirFileId {
        let macro_file = MacroFile { macro_call_id: self, macro_file_kind: kind };
        macro_file.into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// ExpansionInfo mainly describle how to map text range between src and expaned macro
pub struct ExpansionInfo {
    pub arg_map: Vec<(TextRange, TextRange)>,
    pub def_map: Vec<(TextRange, TextRange)>,
}

impl ExpansionInfo {
    pub fn find_range(
        &self,
        from: TextRange,
        (arg_file_id, def_file_id): (HirFileId, HirFileId),
    ) -> Option<(HirFileId, TextRange)> {
        for (src, dest) in &self.arg_map {
            dbg!((src, *dest, "arg_map"));
            if src.is_subrange(&from) {
                dbg!((arg_file_id, *dest));
                return Some((arg_file_id, *dest));
            }
        }

        for (src, dest) in &self.def_map {
            dbg!((src, *dest, "def_map"));
            if src.is_subrange(&from) {
                dbg!((arg_file_id, *dest));
                return Some((def_file_id, *dest));
            }
        }

        None
    }
}

/// `AstId` points to an AST node in any file.
///
/// It is stable across reparses, and can be used as salsa key/value.
// FIXME: isn't this just a `Source<FileAstId<N>>` ?
#[derive(Debug)]
pub struct AstId<N: AstNode> {
    file_id: HirFileId,
    file_ast_id: FileAstId<N>,
}

impl<N: AstNode> Clone for AstId<N> {
    fn clone(&self) -> AstId<N> {
        *self
    }
}
impl<N: AstNode> Copy for AstId<N> {}

impl<N: AstNode> PartialEq for AstId<N> {
    fn eq(&self, other: &Self) -> bool {
        (self.file_id, self.file_ast_id) == (other.file_id, other.file_ast_id)
    }
}
impl<N: AstNode> Eq for AstId<N> {}
impl<N: AstNode> Hash for AstId<N> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        (self.file_id, self.file_ast_id).hash(hasher);
    }
}

impl<N: AstNode> AstId<N> {
    pub fn new(file_id: HirFileId, file_ast_id: FileAstId<N>) -> AstId<N> {
        AstId { file_id, file_ast_id }
    }

    pub fn file_id(&self) -> HirFileId {
        self.file_id
    }

    pub fn to_node(&self, db: &dyn db::AstDatabase) -> N {
        let root = db.parse_or_expand(self.file_id).unwrap();
        db.ast_id_map(self.file_id).get(self.file_ast_id).to_node(&root)
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Source<T> {
    pub file_id: HirFileId,
    pub ast: T,
}

impl<T> Source<T> {
    pub fn map<F: FnOnce(T) -> U, U>(self, f: F) -> Source<U> {
        Source { file_id: self.file_id, ast: f(self.ast) }
    }
    pub fn file_syntax(&self, db: &impl db::AstDatabase) -> SyntaxNode {
        db.parse_or_expand(self.file_id).expect("source created from invalid file")
    }
}
