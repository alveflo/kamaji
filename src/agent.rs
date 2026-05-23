use crate::config::AgentCommands;

/// Resolve the argv to launch. With a non-empty prompt, `{prompt}` is replaced
/// in each part of `with_prompt`; otherwise `no_prompt` is used verbatim.
pub fn build_command(template: &AgentCommands, prompt: Option<&str>) -> Vec<String> {
    match prompt {
        Some(p) if !p.is_empty() => template
            .with_prompt
            .iter()
            .map(|part| part.replace("{prompt}", p))
            .collect(),
        _ => template.no_prompt.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tpl() -> AgentCommands {
        AgentCommands {
            with_prompt: vec!["claude".into(), "{prompt}".into()],
            no_prompt: vec!["claude".into()],
        }
    }

    #[test]
    fn substitutes_prompt() {
        assert_eq!(
            build_command(&tpl(), Some("fix the bug")),
            vec!["claude", "fix the bug"]
        );
    }

    #[test]
    fn empty_or_missing_prompt_uses_no_prompt() {
        assert_eq!(build_command(&tpl(), None), vec!["claude"]);
        assert_eq!(build_command(&tpl(), Some("")), vec!["claude"]);
    }
}
