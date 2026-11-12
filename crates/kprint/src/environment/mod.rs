mod real_environment;
pub use kprint_platform::environment::*;
#[cfg(test)]
pub use kprint_test_support::environment::TestConfigFileBuilder;
#[cfg(test)]
pub use kprint_test_support::environment::TestEnvironment;
#[cfg(test)]
pub use kprint_test_support::environment::TestEnvironmentBuilder;
#[cfg(test)]
pub use kprint_test_support::environment::TestInfoFileBuilder;
#[cfg(test)]
pub use kprint_test_support::environment::TestInfoFileConfigItem;
#[cfg(test)]
pub use kprint_test_support::environment::TestInfoFileMatch;
#[cfg(test)]
pub use kprint_test_support::environment::TestInfoFileNpm;
#[cfg(test)]
pub use kprint_test_support::environment::TestInfoFilePlugin;
pub use real_environment::*;

/// Interactive init prompts stay in the CLI rather than the host library API.
pub trait InitEnvironment: Environment {
  fn prompt_terminal(&self) -> Option<Box<dyn crate::terminal::PromptTerminal + '_>>;
  #[cfg(test)]
  fn set_prompt_keys(&self, keys: &[&str], size: Option<crate::terminal::TerminalSize>) -> anyhow::Result<()> {
    let terminal = crate::terminal::ScriptedPromptTerminal::new(keys, size)?;
    INIT_PROMPT.with(|slot| *slot.borrow_mut() = Some(terminal));
    Ok(())
  }
  #[cfg(test)]
  fn take_prompt_screens(&self) -> Vec<crate::terminal::PromptScreen> {
    INIT_PROMPT.with(|slot| slot.borrow().as_ref().map(|terminal| terminal.take_screens()).unwrap_or_default())
  }
}
#[cfg(test)]
thread_local! {
  static INIT_PROMPT: std::cell::RefCell<Option<crate::terminal::ScriptedPromptTerminal>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
impl InitEnvironment for TestEnvironment {
  fn prompt_terminal(&self) -> Option<Box<dyn crate::terminal::PromptTerminal + '_>> {
    INIT_PROMPT.with(|slot| {
      slot
        .borrow()
        .clone()
        .map(|terminal| Box::new(terminal) as Box<dyn crate::terminal::PromptTerminal>)
    })
  }
}
