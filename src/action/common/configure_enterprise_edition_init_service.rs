use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{span, Span};

use crate::action::{ActionError, ActionErrorKind, ActionTag, StatefulAction};
use crate::execute_command;

use crate::action::{common::ConfigureInitService, Action, ActionDescription};
use crate::settings::InitSystem;

const DARWIN_ENTERPRISE_EDITION_DAEMON_DEST: &str =
    "/Library/LaunchDaemons/systems.determinate.nix-daemon.plist";
const DARWIN_LAUNCHD_DOMAIN: &str = "system";
const SERVICE_DEST: &str = "/etc/systemd/system/nix-daemon.service";
const DETERMINATE_NIX_EE_SERVICE_SRC: &str = "/nix/determinate/nix-daemon.service";
const DARWIN_NIX_DAEMON_SOURCE: &str =
    "/nix/var/nix/profiles/default/Library/LaunchDaemons/org.nixos.nix-daemon.plist";
const DARWIN_ENTERPRISE_EDITION_SERVICE_NAME: &str = "systems.determinate.nix-daemon";

/**
Configure the init to run the Nix daemon
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
pub struct ConfigureEnterpriseEditionInitService {
    start_daemon: bool,
    // FIXME(cole-h): add to tracing stuff
    configure_init_service: StatefulAction<ConfigureInitService>,
}

impl ConfigureEnterpriseEditionInitService {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        init: InitSystem,
        start_daemon: bool,
    ) -> Result<StatefulAction<Self>, ActionError> {
        let service_src: Option<PathBuf> = match init {
            InitSystem::Launchd => {
                // We'll write it out down in the execute step
                None
            },
            // FIXME(cole-h): should this be None, or are we writing the service to this location and then copying it to its destination..?
            InitSystem::Systemd => Some(DETERMINATE_NIX_EE_SERVICE_SRC.into()),
            InitSystem::None => None,
        };
        let service_dest: Option<PathBuf> = match init {
            InitSystem::Launchd => Some(DARWIN_ENTERPRISE_EDITION_DAEMON_DEST.into()),
            InitSystem::Systemd => Some(SERVICE_DEST.into()),
            InitSystem::None => None,
        };
        let service_name: Option<String> = match init {
            InitSystem::Launchd => Some(DARWIN_ENTERPRISE_EDITION_SERVICE_NAME.into()),
            _ => None,
        };

        let configure_init_service = ConfigureInitService::plan(
            InitSystem::Launchd,
            start_daemon,
            service_src,
            service_dest,
            service_name,
        )
        .await
        .map_err(Self::error)?;

        Ok(Self {
            start_daemon,
            configure_init_service,
        }
        .into())
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "configure_enterprise_edition_init_service")]
impl Action for ConfigureEnterpriseEditionInitService {
    fn action_tag() -> ActionTag {
        ActionTag("configure_enterprise_edition_init_service")
    }
    fn tracing_synopsis(&self) -> String {
        "Configure the Determinate Nix Enterprise Edition daemon related settings with launchctl"
            .to_string()
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "configure_enterprise_edition_init_service"
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        let mut explanation = vec![format!("Create `{DARWIN_ENTERPRISE_EDITION_DAEMON_DEST}`")];
        if self.start_daemon {
            explanation.push(format!(
                "Run `launchctl bootstrap {DARWIN_ENTERPRISE_EDITION_DAEMON_DEST}`"
            ));
        }

        vec![ActionDescription::new(self.tracing_synopsis(), explanation)]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> Result<(), ActionError> {
        let Self {
            start_daemon,
            configure_init_service,
        } = self;

        let daemon_file = DARWIN_ENTERPRISE_EDITION_DAEMON_DEST;

        {
            // This is the only part that is actually different from configure_init_service, beyond variable parameters.

            let generated_plist = generate_plist();

            let mut options = tokio::fs::OpenOptions::new();
            options.create(true).write(true).read(true);

            let mut file = options
                .open(&daemon_file)
                .await
                .map_err(|e| Self::error(ActionErrorKind::Open(PathBuf::from(daemon_file), e)))?;

            let mut buf = Vec::new();
            plist::to_writer_xml(&mut buf, &generated_plist).map_err(Self::error)?;
            file.write_all(&buf)
                .await
                .map_err(|e| Self::error(ActionErrorKind::Write(PathBuf::from(daemon_file), e)))?;
        }

        configure_init_service
            .try_execute()
            .await
            .map_err(Self::error)?;

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            "Unconfigure Nix daemon related settings with launchctl".to_string(),
            vec![format!(
                "Run `launchctl bootout {DARWIN_ENTERPRISE_EDITION_DAEMON_DEST}`"
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> Result<(), ActionError> {
        execute_command(
            Command::new("launchctl")
                .process_group(0)
                .arg("bootout")
                .arg(
                    [
                        DARWIN_LAUNCHD_DOMAIN,
                        DARWIN_ENTERPRISE_EDITION_SERVICE_NAME,
                    ]
                    .join("/"),
                ),
        )
        .await
        .map_err(Self::error)?;

        Ok(())
    }
}

#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum ConfigureEnterpriseEditionNixDaemonServiceError {}

#[derive(Deserialize, Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct DeterminateNixDaemonPlist {
    label: String,
    program: String,
    keep_alive: bool,
    run_at_load: bool,
    standard_error_path: String,
    standard_out_path: String,
    soft_resource_limits: ResourceLimits,
}

#[derive(Deserialize, Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct ResourceLimits {
    number_of_files: usize,
}

fn generate_plist() -> DeterminateNixDaemonPlist {
    DeterminateNixDaemonPlist {
        keep_alive: true,
        run_at_load: true,
        label: "systems.determinate.nix-daemon".into(),
        program: "/usr/local/bin/determinate-nix-ee".into(),
        standard_error_path: "/var/log/determinate-nix-daemon.log".into(),
        standard_out_path: "/var/log/determinate-nix-daemon.log".into(),
        soft_resource_limits: ResourceLimits {
            number_of_files: 1048576,
        },
    }
}
