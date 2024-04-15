use std::{collections::{HashMap, HashSet}, env, ffi::OsString, fs, process::{Command, Stdio}, str::FromStr};

use anyhow::bail;
use camino::{Utf8Path, Utf8PathBuf};
use dexterous_developer_types::{cargo_path_utils::dylib_path, Target, TargetBuildSettings};
use object::Object;
use tracing::{error, info};
use which::{which, which_all, WhichConfig};

use crate::types::{
    BuildOutputMessages, Builder, BuilderIncomingMessages, BuilderOutgoingMessages, HashedFileRecord,
};

pub struct SimpleBuilder {
    target: Target,
    settings: TargetBuildSettings,
    incoming: tokio::sync::mpsc::UnboundedSender<BuilderIncomingMessages>,
    outgoing: tokio::sync::broadcast::Sender<BuilderOutgoingMessages>,
    output: tokio::sync::broadcast::Sender<BuildOutputMessages>,
    #[allow(dead_code)]
    handle: tokio::task::JoinHandle<()>,
}

fn build(
    target: Target,
    TargetBuildSettings {
        package_or_example,
        features,
        ..
    }: TargetBuildSettings,
    sender: tokio::sync::broadcast::Sender<BuildOutputMessages>,
) -> Result<(), anyhow::Error> {
    let mut cargo = Command::new("cargo");
    cargo
        .env_remove("LD_DEBUG")
        .env("RUSTFLAGS", "-Cprefer-dynamic")
        .env("CARGO_TARGET_DIR", format!("./target/hot-reload/{target}"))
        .arg("build")
        .arg("--lib")
        .arg("--message-format=json-render-diagnostics")
        .arg("--profile")
        .arg("dev")
        .arg("--target")
        .arg(target.to_string());

    match package_or_example {
        dexterous_developer_types::PackageOrExample::DefaulPackage => {}
        dexterous_developer_types::PackageOrExample::Package(package) => {
            cargo.arg("-p").arg(package.as_str());
        }
        dexterous_developer_types::PackageOrExample::Example(example) => {
            cargo.arg("--example").arg(example.as_str());
        }
    }

    if !features.is_empty() {
        cargo.arg("--features");
        cargo.arg(features.join(",").as_str());
    }

    let mut  child = cargo.stdout(Stdio::piped()).spawn()?;
    
    
    let reader = std::io::BufReader::new(
        child
            .stdout
            .take().ok_or(anyhow::Error::msg("no stdout"))?
    );

    let mut succeeded = false;  

    let mut artifacts = Vec::with_capacity(20);
    
    for msg in cargo_metadata::Message::parse_stream(reader) {
        let message = msg?;
        match &message {
            cargo_metadata::Message::CompilerArtifact(artifact) => {
                if artifact.target.crate_types.iter().any(|v| v == "dylib") {
                    artifacts.push(artifact.clone());
                }
            },
            cargo_metadata::Message::BuildFinished(finished) => {
                info!("Build Finished: {finished:?}");
                succeeded = finished.success;
            }
            msg => info!("Compiler: {msg:?}"),
        }
    }

    let result = child.wait()?;

    if !result.success() || !succeeded {
        error!("Build Failed");
        bail!("Failed to build");
    }

    let mut libraries = HashMap::<String, Utf8PathBuf>::with_capacity(20);
    let mut root_dir = HashSet::new();

    for artifact in artifacts.iter() {
        for path in artifact.filenames.iter() {
            if let Some(ext) = path.extension() {
                if ext == target.dynamic_lib_extension() {
                    if let Some(name) = path.file_name() {
                        let path = path.canonicalize_utf8()?;
                        let parent = path.parent().ok_or_else(|| anyhow::Error::msg("file has no parent"))?;
                        root_dir.insert(parent.to_path_buf());
                        libraries.insert(name.to_string(), path);
                    }
                }
            }
        }
    }

    let initial_libraries = libraries.iter().map(|(name,path)| (name.clone(), path.clone())).collect::<Vec<_>>();

    let mut path_var = match env::var_os("PATH") {
        Some(var) => env::split_paths(&var)
            .filter_map(|p| Utf8PathBuf::try_from(p).ok())
            .collect(),
        None => Vec::new()
    };
    let mut dylib_paths = dylib_path();
    let mut root_dirs = root_dir.into_iter().collect::<Vec<_>>();
    path_var.append(&mut dylib_paths);
    path_var.append(&mut root_dirs);
    let path_var = env::join_paths(&path_var)?;

    let mut dependencies = HashMap::new();

    for (name, library) in initial_libraries.iter() {
        process_dependencies_recursive(&path_var, &mut libraries, &mut dependencies, &name, library)?;
    }

    for (library, local_path) in libraries.iter() {
        let file = std::fs::read(local_path)?;
        let hash = blake3::hash(&file);

        let _ = sender.send(BuildOutputMessages::LibraryUpdated(HashedFileRecord {
            local_path: local_path.clone(),
            relative_path: Utf8PathBuf::from(format!("./{library}")),
            hash: hash.as_bytes().to_owned(),
            dependencies: dependencies.get(library).cloned().unwrap_or_default()
        }));
    }
    Ok(())
}

fn process_dependencies_recursive(path_var: &OsString, libraries: &mut HashMap<String, Utf8PathBuf>, dependencies: &mut HashMap<String, Vec<Utf8PathBuf>>, current_library_name: &str, current_library: &Utf8Path) -> Result<(), anyhow::Error> {
      let file = fs::read(current_library)?;
      let file = object::File::parse(&*file)?;
      let imports =  file.imports()?;
      let dependency_vec = imports.iter().map(|v| std::str::from_utf8(v.library()).map(|v| v.to_owned())).collect::<Result<Vec<_>, _>>()?;
      for library_name in dependency_vec.iter() {        
        if libraries.contains_key(library_name) {
            continue;
        }
        let which = WhichConfig::default().custom_path_list(path_var.clone()).binary_name(OsString::from_str(library_name)?);
        let Ok(library_path) = which.first_result() else {
            eprintln!("Couldn't find library with name {library_name}");
            continue;
        };
        let library_path = Utf8PathBuf::try_from(library_path)?;
        libraries.insert(library_name.to_string(), library_path);
      }
      dependencies.insert(current_library_name.to_string(), dependency_vec.iter().map(|v| Utf8PathBuf::from(format!("./{v}"))).collect());
      Ok(())
}

impl SimpleBuilder {
    pub fn new(target: Target, settings: TargetBuildSettings) -> Self {
        let (incoming, mut incoming_rx) = tokio::sync::mpsc::unbounded_channel();
        let (outgoing_tx, _) = tokio::sync::broadcast::channel(100);
        let (output_tx, _) = tokio::sync::broadcast::channel(100);

        let handle = {
            let outgoing_tx = outgoing_tx.clone();
            let output_tx = output_tx.clone();
            let settings = settings.clone();
            tokio::spawn(async move {
                let mut should_build = false;

                while let Some(recv) = incoming_rx.recv().await {
                    match recv {
                        BuilderIncomingMessages::RequestBuild => {
                            should_build = true;
                            let _ = outgoing_tx.send(BuilderOutgoingMessages::BuildStarted);
                            let _ = build(target, settings.clone(), output_tx.clone());
                        }
                        BuilderIncomingMessages::CodeChanged => {
                            if should_build {
                                let _ = outgoing_tx.send(BuilderOutgoingMessages::BuildStarted);
                                let _ = build(target, settings.clone(), output_tx.clone());
                            }
                        }
                        BuilderIncomingMessages::AssetChanged(asset) => {
                            let _ = output_tx.send(BuildOutputMessages::AssetUpdated(asset));
                        }
                    }
                }
            })
        };

        Self {
            settings,
            target,
            incoming,
            outgoing: outgoing_tx,
            output: output_tx,
            handle,
        }
    }
}

impl Builder for SimpleBuilder {
    fn target(&self) -> Target {
        self.target
    }

    fn incoming_channel(
        &self,
    ) -> tokio::sync::mpsc::UnboundedSender<crate::types::BuilderIncomingMessages> {
        self.incoming.clone()
    }

    fn outgoing_channel(
        &self,
    ) -> (
        tokio::sync::broadcast::Receiver<crate::types::BuilderOutgoingMessages>,
        tokio::sync::broadcast::Receiver<crate::types::BuildOutputMessages>,
    ) {
        (self.outgoing.subscribe(), self.output.subscribe())
    }

    fn root_lib_name(&self) -> Option<camino::Utf8PathBuf> {
        None
    }

    fn get_code_subscriptions(&self) -> Vec<camino::Utf8PathBuf> {
        self.settings.code_watch_folders.clone()
    }

    fn get_asset_subscriptions(&self) -> Vec<camino::Utf8PathBuf> {
        self.settings.asset_folders.clone()
    }
}
