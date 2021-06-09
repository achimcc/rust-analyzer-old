//! Fully type-check project and print various stats, like the number of type
//! errors.

use crossbeam_channel::{unbounded, Receiver};
use ide_db::base_db::CrateGraph;
use ide::{Change};
use project_model::{
    BuildDataCollector, CargoConfig, ProcMacroClient, ProjectManifest, ProjectWorkspace,
};
use std::path::Path;

use crate::cli::{load_cargo::LoadCargoConfig, Result};

use crate::reload::{SourceRootConfig};

use vfs::{loader::Handle, AbsPath, AbsPathBuf};

pub struct CreateJsonCmd {}

impl CreateJsonCmd {
    /// Execute with e.g.
    /// ```no_compile
    /// cargo run --bin rust-analyzer create-json ../ink/examples/flipper/Cargo.toml
    /// ```
    pub fn run(self, root: &Path) -> Result<()> {
        println!("Running! {:?}", root);
        let mut cargo_config = CargoConfig::default();
        cargo_config.no_sysroot = false;
        let root = AbsPathBuf::assert(std::env::current_dir()?.join(root));

        let root = AbsPath::assert(&root);
        let root = ProjectManifest::discover_single(root)?;
        let ws = ProjectWorkspace::load(root, &cargo_config, &|_| {})?;

        let load_cargo_config = LoadCargoConfig {
            load_out_dirs_from_check: false,
            wrap_rustc: false,
            with_proc_macro: false,
        };

        let crate_graph = get_crate_graph(ws, &load_cargo_config, &|_| {})?;

        let json = serde_json::to_string(&crate_graph).expect("serialization must work");
        // println!("json:\n{}", json);

        // deserialize from json string
        let deserialized_crate_graph: CrateGraph =
            serde_json::from_str(&json).expect("deserialization must work");
        assert_eq!(
            crate_graph, deserialized_crate_graph,
            "Deserialized `CrateGraph` is not equal!"
        );

        // Missing: Create a new `Change` object.
        //
        // `serde::Serialize` and `serde::Deserialize` are already supported by `Change`.
        // So this should work out of the box after the object has been created:
        //
        // ```
        // let json = serde_json::to_string(&change).expect("`Change` serialization must work");
        // println!("change json:\n{}", json);
        // let deserialized_change: Change = serde_json::from_str(&json).expect("`Change` deserialization must work");
        // assert_eq!(change, deserialized_change, "Deserialized `Change` is not equal!");
        // ```

        Ok(())
    }
}

fn get_crate_graph(
    ws: ProjectWorkspace,
    config: &LoadCargoConfig,
    progress: &dyn Fn(String),
) -> Result<CrateGraph> {
    let (sender, _receiver) = unbounded();
    let mut vfs = vfs::Vfs::default();
    let mut loader = {
        let loader =
            vfs_notify::NotifyHandle::spawn(Box::new(move |msg| sender.send(msg).unwrap()));
        Box::new(loader)
    };

    let proc_macro_client = if config.with_proc_macro {
        let path = std::env::current_exe()?;
        Some(ProcMacroClient::extern_process(path, &["proc-macro"]).unwrap())
    } else {
        None
    };

    let build_data = if config.load_out_dirs_from_check {
        let mut collector = BuildDataCollector::new(config.wrap_rustc);
        ws.collect_build_data_configs(&mut collector);
        Some(collector.collect(progress)?)
    } else {
        None
    };

    let crate_graph = ws.to_crate_graph(
        build_data.as_ref(),
        proc_macro_client.as_ref(),
        &mut |path: &AbsPath| {
            let contents = loader.load_sync(path);
            let path = vfs::VfsPath::from(path.to_path_buf());
            vfs.set_file_contents(path.clone(), contents);
            vfs.file_id(&path)
        },
    );

    Ok(crate_graph)
}

fn get_change(
    crate_graph: CrateGraph,
    source_root_config: SourceRootConfig,
    vfs: &mut vfs::Vfs,
    receiver: &Receiver<vfs::loader::Message>,
) -> Change {
    let lru_cap = std::env::var("RA_LRU_CAP").ok().and_then(|it| it.parse::<usize>().ok());
    let mut analysis_change = Change::new();

    // wait until Vfs has loaded all roots
    for task in receiver {
        match task {
            vfs::loader::Message::Progress { n_done, n_total, config_version: _ } => {
                if n_done == n_total {
                    break;
                }
            }
            vfs::loader::Message::Loaded { files } => {
                for (path, contents) in files {
                    vfs.set_file_contents(path.into(), contents);
                }
            }
        }
    }
    let changes = vfs.take_changes();
    for file in changes {
        if file.exists() {
            let contents = vfs.file_contents(file.file_id).to_vec();
            if let Ok(text) = String::from_utf8(contents) {
                analysis_change.change_file(file.file_id, Some(Arc::new(text)))
            }
        }
    }
    let source_roots = source_root_config.partition(&vfs);
    analysis_change.set_roots(source_roots);

    analysis_change.set_crate_graph(crate_graph);

    analysis_change
}
