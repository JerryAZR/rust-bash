//! Static command analysis: walks a parsed program and collects command names
//! without executing anything. Used by `RustBash::analyze_commands` for
//! parse-time (pre-flight) checks of which commands a script would dispatch.

use brush_parser::ast;

/// Literal command names and function definitions collected from an AST.
#[derive(Debug, Default)]
pub(crate) struct CollectedCommands {
    /// Every literal simple-command name, deduplicated in first-encountered
    /// order. Names that require expansion (variables, command substitution,
    /// globs, quotes) are not statically knowable and are omitted.
    pub commands: Vec<String>,
    /// Names of functions defined in the analyzed script, deduplicated.
    pub defined_functions: Vec<String>,
}

/// Collect literal command names and function definitions from a parsed program.
///
/// Walks the full AST — top-level lists, pipelines, function bodies, subshells,
/// and compound-command bodies (if/for/while/until/case/brace groups) — without
/// executing anything or touching interpreter state.
pub(crate) fn collect_commands(program: &ast::Program) -> CollectedCommands {
    let mut out = CollectedCommands::default();
    for list in &program.complete_commands {
        collect_list(list, &mut out);
    }
    out
}

fn push_unique(list: &mut Vec<String>, name: String) {
    if !list.contains(&name) {
        list.push(name);
    }
}

/// A word is a statically knowable command name only when it is a plain
/// literal: expansions (`$`, backticks), globs, quotes, and other shell
/// metacharacters make the resolved name depend on runtime state.
fn literal_command_name(word: &ast::Word) -> Option<&str> {
    let name = word.value.as_str();
    let is_plain = !name.is_empty()
        && name.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '%' | '@' | '+')
        });
    if is_plain { Some(name) } else { None }
}

fn collect_list(list: &ast::CompoundList, out: &mut CollectedCommands) {
    for item in &list.0 {
        let ast::CompoundListItem(and_or_list, _) = item;
        collect_pipeline(&and_or_list.first, out);
        for and_or in &and_or_list.additional {
            let pipeline = match and_or {
                ast::AndOr::And(p) => p,
                ast::AndOr::Or(p) => p,
            };
            collect_pipeline(pipeline, out);
        }
    }
}

fn collect_pipeline(pipeline: &ast::Pipeline, out: &mut CollectedCommands) {
    for command in &pipeline.seq {
        collect_command(command, out);
    }
}

fn collect_command(command: &ast::Command, out: &mut CollectedCommands) {
    match command {
        ast::Command::Simple(simple) => collect_simple_command(simple, out),
        ast::Command::Compound(compound, _) => collect_compound_command(compound, out),
        ast::Command::Function(func_def) => {
            if let Some(name) = literal_command_name(&func_def.fname) {
                push_unique(&mut out.defined_functions, name.to_string());
            }
            collect_compound_command(&func_def.body.0, out);
        }
        // `[[ ... ]]` evaluates a test expression; it dispatches no commands.
        ast::Command::ExtendedTest(_, _) => {}
    }
}

fn collect_simple_command(cmd: &ast::SimpleCommand, out: &mut CollectedCommands) {
    if let Some(name) = cmd.word_or_name.as_ref().and_then(literal_command_name) {
        push_unique(&mut out.commands, name.to_string());
    }
    // Process substitutions in the prefix/suffix contain runnable commands.
    for item in prefix_suffix_items(cmd) {
        if let ast::CommandPrefixOrSuffixItem::ProcessSubstitution(_, subshell) = item {
            collect_list(&subshell.list, out);
        }
    }
}

fn prefix_suffix_items(cmd: &ast::SimpleCommand) -> Vec<&ast::CommandPrefixOrSuffixItem> {
    let mut items = Vec::new();
    if let Some(prefix) = &cmd.prefix {
        items.extend(prefix.0.iter());
    }
    if let Some(suffix) = &cmd.suffix {
        items.extend(suffix.0.iter());
    }
    items
}

fn collect_compound_command(compound: &ast::CompoundCommand, out: &mut CollectedCommands) {
    match compound {
        ast::CompoundCommand::IfClause(if_clause) => {
            collect_list(&if_clause.condition, out);
            collect_list(&if_clause.then, out);
            if let Some(elses) = &if_clause.elses {
                for else_clause in elses {
                    if let Some(condition) = &else_clause.condition {
                        collect_list(condition, out);
                    }
                    collect_list(&else_clause.body, out);
                }
            }
        }
        ast::CompoundCommand::ForClause(for_clause) => collect_list(&for_clause.body.list, out),
        ast::CompoundCommand::ArithmeticForClause(for_clause) => {
            collect_list(&for_clause.body.list, out)
        }
        ast::CompoundCommand::WhileClause(wc) | ast::CompoundCommand::UntilClause(wc) => {
            collect_list(&wc.0, out);
            collect_list(&wc.1.list, out);
        }
        ast::CompoundCommand::BraceGroup(bg) => collect_list(&bg.list, out),
        ast::CompoundCommand::Subshell(sub) => collect_list(&sub.list, out),
        ast::CompoundCommand::CaseClause(case_clause) => {
            for case_item in &case_clause.cases {
                if let Some(cmd) = &case_item.cmd {
                    collect_list(cmd, out);
                }
            }
        }
        // Arithmetic commands evaluate expressions; they dispatch no commands.
        ast::CompoundCommand::Arithmetic(_) => {}
        // Unreachable: the pinned brush-parser revision parses `coproc` as a
        // simple command, so no Coprocess AST node is ever produced.
        ast::CompoundCommand::Coprocess(coproc) => collect_command(&coproc.body, out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::parse;

    fn names(script: &str) -> CollectedCommands {
        collect_commands(&parse(script).unwrap())
    }

    #[test]
    fn collects_top_level_command_names() {
        let out = names("echo hi; cat /etc/hosts");
        assert_eq!(out.commands, vec!["echo", "cat"]);
    }

    #[test]
    fn deduplicates_command_names_in_encounter_order() {
        let out = names("grep -n foo; echo hi; grep -n bar");
        assert_eq!(out.commands, vec!["grep", "echo"]);
    }

    #[test]
    fn skips_assignment_only_commands() {
        let out = names("FOO=bar");
        assert!(out.commands.is_empty());
    }

    #[test]
    fn skips_dynamic_command_names() {
        let out = names("$cmd status");
        assert!(out.commands.is_empty());
    }

    #[test]
    fn collects_names_inside_function_bodies() {
        let out = names("deploy() {\n  git pull\n  make -j4\n}\ndeploy");
        assert_eq!(out.commands, vec!["git", "make", "deploy"]);
        assert_eq!(out.defined_functions, vec!["deploy"]);
    }

    #[test]
    fn collects_names_inside_compound_bodies() {
        let out = names("if true; then alpha one; elif false; then beta two; else gamma three; fi");
        assert_eq!(
            out.commands,
            vec!["true", "alpha", "false", "beta", "gamma"]
        );
    }

    #[test]
    fn collects_names_inside_loops_and_case() {
        let out = names(
            "for f in a b; do delta $f; done\nwhile false; do epsilon; done\ncase x in *) zeta;; esac",
        );
        assert_eq!(out.commands, vec!["delta", "false", "epsilon", "zeta"]);
    }

    #[test]
    fn collects_names_inside_pipelines_and_subshells() {
        let out = names("(eta | theta) && iota");
        assert_eq!(out.commands, vec!["eta", "theta", "iota"]);
    }

    #[test]
    fn collects_names_inside_process_substitution() {
        let out = names("diff <(sort a.txt) <(sort b.txt)");
        assert_eq!(out.commands, vec!["diff", "sort"]);
    }

    #[test]
    fn ignores_arithmetic_and_extended_tests() {
        let out = names("(( x = 1 + 2 )); [[ -n \"$v\" ]]");
        assert!(out.commands.is_empty());
    }
}
