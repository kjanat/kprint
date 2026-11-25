use anyhow::Result;
use std::rc::Rc;
use thiserror::Error;

use crate::arg_parser::ParseArgsError;
use crate::commands::CheckError;
use crate::commands::InitConfigFileOptions;
use crate::configuration::ResolveConfigError;
use crate::paths::NoFilesFoundError;
use crate::plugins::PluginResolver;

use crate::arg_parser::CliArgs;
use crate::arg_parser::ConfigSubCommand;
use crate::arg_parser::SubCommand;
use crate::commands;
use crate::resolution::NoPluginsFoundError;
use crate::resolution::ResolvePluginsError;

#[derive(Debug, Error)]
#[error("{inner:#}")]
pub struct AppError {
  pub inner: anyhow::Error,
  pub exit_code: i32,
}

impl From<anyhow::Error> for AppError {
  fn from(inner: anyhow::Error) -> Self {
    let exit_code = if inner.is::<ParseArgsError>() {
      10
    } else if inner.is::<ResolveConfigError>() {
      11
    } else if inner.is::<ResolvePluginsError>() {
      12
    } else if inner.is::<NoPluginsFoundError>() {
      13
    } else if inner.is::<NoFilesFoundError>() {
      14
    } else if inner.is::<CheckError>() {
      20
    } else {
      1
    };
    AppError { inner, exit_code }
  }
}

impl From<ParseArgsError> for AppError {
  fn from(inner: ParseArgsError) -> Self {
    anyhow::Error::from(inner).into()
  }
}

impl From<ResolveConfigError> for AppError {
  fn from(inner: ResolveConfigError) -> Self {
    anyhow::Error::from(inner).into()
  }
}

impl From<ResolvePluginsError> for AppError {
  fn from(inner: ResolvePluginsError) -> Self {
    anyhow::Error::from(inner).into()
  }
}

impl From<NoPluginsFoundError> for AppError {
  fn from(inner: NoPluginsFoundError) -> Self {
    anyhow::Error::from(inner).into()
  }
}

impl From<NoFilesFoundError> for AppError {
  fn from(inner: NoFilesFoundError) -> Self {
    anyhow::Error::from(inner).into()
  }
}

impl From<CheckError> for AppError {
  fn from(inner: CheckError) -> Self {
    anyhow::Error::from(inner).into()
  }
}

pub async fn run_cli<TEnvironment: crate::environment::InitEnvironment>(
  args: &CliArgs,
  environment: &TEnvironment,
  plugin_resolver: &Rc<PluginResolver<TEnvironment>>,
) -> Result<()> {
  plugin_resolver.set_reload_plugins(args.reload.plugins);
  match &args.sub_command {
    SubCommand::Help(help_text) => commands::output_help(args, environment, plugin_resolver, help_text).await,
    SubCommand::SubcommandHelp(help_text) => commands::output_subcommand_help(environment, help_text),
    SubCommand::License => commands::output_license(args, environment, plugin_resolver).await,
    SubCommand::EditorInfo => {
      commands::legacy_editor_policy(environment)
        .scope(commands::output_editor_info(args, environment, plugin_resolver))
        .await
    }
    SubCommand::EditorService(cmd) => {
      commands::legacy_editor_policy(environment)
        .scope(commands::run_editor_service(args, environment, plugin_resolver, cmd))
        .await
    }
    SubCommand::Lsp => commands::run_language_server(args, environment, plugin_resolver).await,
    SubCommand::ClearCache => commands::clear_cache(environment),
    SubCommand::Config(cmd) => match cmd {
      ConfigSubCommand::Init {
        preferences,
        global,
        yes,
        minimum_dependency_age,
      } => {
        if *preferences {
          let loader = Rc::new(crate::configuration::PluginResolverOptionsLoader::new(
            environment.clone(),
            plugin_resolver.clone(),
          ));
          let result = crate::configuration::prompt_for_init_preferences(environment, Some(loader)).await;
          plugin_resolver.clear_and_shutdown_initialized().await;
          return result;
        }
        commands::init_config_file(
          environment,
          plugin_resolver,
          InitConfigFileOptions {
            global: *global,
            config_arg: args.config.as_ref().and_then(|config| config.maybe_path_or_url()),
            non_interactive: *yes,
            minimum_dependency_age: minimum_dependency_age.clone(),
          },
        )
        .await
      }
      ConfigSubCommand::Add {
        names,
        no_version,
        package_json,
        checksum,
        minimum_dependency_age,
      } => {
        commands::add_plugin_config_file(
          args,
          commands::AddPluginsOptions {
            plugin_names_or_urls: names,
            no_version: *no_version,
            update_package_json: *package_json,
            checksum: *checksum,
            minimum_dependency_age: minimum_dependency_age.clone(),
          },
          environment,
          plugin_resolver,
        )
        .await
      }
      ConfigSubCommand::Update {
        yes,
        dry_run,
        minimum_dependency_age,
      } => {
        commands::update_plugins_config_file(
          args,
          environment,
          plugin_resolver,
          commands::UpdatePluginsOptions {
            yes_to_prompts: *yes,
            dry_run: *dry_run,
            minimum_dependency_age: minimum_dependency_age.clone(),
          },
        )
        .await
      }
      ConfigSubCommand::Install(cmd) => commands::install_plugins(cmd, args, environment, plugin_resolver).await,
      ConfigSubCommand::Edit => commands::edit_config_file(args, environment).await,
    },
    SubCommand::Version => commands::output_version(environment),
    SubCommand::StdInFmt(cmd) => commands::stdin_fmt(cmd, args, environment, plugin_resolver).await,
    SubCommand::OutputResolvedConfig(cmd) => commands::output_resolved_config(cmd, args, environment, plugin_resolver).await,
    SubCommand::Schema(cmd) => commands::output_config_schema(args, cmd, environment, plugin_resolver).await,
    SubCommand::IncrementalState => commands::incremental_state(args, environment, plugin_resolver).await,
    SubCommand::OutputFilePaths(cmd) => commands::output_file_paths(cmd, args, environment, plugin_resolver).await,
    SubCommand::OutputFormatTimes(cmd) => commands::output_format_times(cmd, args, environment, plugin_resolver).await,
    SubCommand::Check(cmd) => commands::check(cmd, args, environment, plugin_resolver).await,
    SubCommand::Fmt(cmd) => commands::format(cmd, args, environment, plugin_resolver).await,
    SubCommand::Completions(shell) => commands::completions(shell.to_owned(), environment),
    SubCommand::Upgrade => commands::upgrade(environment).await,
    #[cfg(target_os = "windows")]
    SubCommand::Hidden(hidden_command) => match hidden_command {
      crate::arg_parser::HiddenSubCommand::WindowsInstall(install_path) => commands::handle_windows_install(environment, install_path),
      crate::arg_parser::HiddenSubCommand::WindowsUninstall(install_path) => commands::handle_windows_uninstall(environment, install_path),
    },
  }
}
