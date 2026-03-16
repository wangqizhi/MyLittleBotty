pub(crate) const TOOL_SYSTEM_PROMPT: &str = include_str!("tool-system.md");
pub(crate) const ROLE_LEADER_SYSTEM_PROMPT: &str = include_str!("role-leader-system.md");
pub(crate) const ROLE_PAPERWORK_SYSTEM_PROMPT: &str = include_str!("role-paperwork-system.md");
pub(crate) const ROLE_ALL_IN_ONE_SYSTEM_PROMPT: &str = include_str!("role-all-in-one-system.md");
pub(crate) const ROLE_CODER_SYSTEM_PROMPT: &str = include_str!("role-coder-system.md");
pub(crate) const ROLE_INFO_SEARCHER_SYSTEM_PROMPT: &str =
    include_str!("role-info-searcher-system.md");
pub(crate) const BROWSER_PROCEDURE_SYSTEM_PROMPT: &str =
    include_str!("browser-procedure-system.md");
pub(crate) const REMEMBER_SYSTEM_PROMPT: &str = include_str!("remember-system.md");
pub(crate) const REMEMBER_UPDATE_PROMPT: &str = include_str!("remember-update.md");
pub(crate) const REMEMBER_INIT_PROMPT: &str = include_str!("remember-init.md");
pub(crate) const REMEMBER_COMPRESS_PROMPT: &str = include_str!("remember-compress.md");
pub(crate) const REMINDER_SYSTEM_PROMPT: &str = include_str!("reminder-system.md");
pub(crate) const REMINDER_USER_PROMPT: &str = include_str!("reminder-user.md");

pub(crate) fn render(template: &str, replacements: &[(&str, &str)]) -> String {
    let mut output = template.to_string();
    for (key, value) in replacements {
        let placeholder = format!("{{{{{key}}}}}");
        output = output.replace(&placeholder, value);
    }
    output
}
