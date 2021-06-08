//! Fully type-check project and print various stats, like the number of type
//! errors.

use std::{path::Path};
use project_model::{
    BuildDataCollector, CargoConfig, ProcMacroClient, ProjectManifest, ProjectWorkspace,
};
use ide_db::base_db::CrateGraph;
use crossbeam_channel::{unbounded};

use crate::cli::{
    load_cargo::LoadCargoConfig,
    Result
};

use vfs::{loader::Handle, AbsPath, AbsPathBuf};

use ide_db::base_db::{CrateId};

pub struct CreateJsonCmd {
}

impl CreateJsonCmd {
    pub fn run(self, root: &Path,) -> Result<()>{
        println!("Running! {:?}", root);
        let mut cargo_config = CargoConfig::default();
        cargo_config.no_sysroot = false;
        let root = AbsPathBuf::assert(std::env::current_dir()?.join(root));
        let root = ProjectManifest::discover_single(&root)?;
        let ws = ProjectWorkspace::load(root, &cargo_config, &|_| {})?;

        let load_cargo_config = LoadCargoConfig {
            load_out_dirs_from_check: false,
            wrap_rustc: false,
            with_proc_macro: false,
        };

        let crate_graph = get_crate_graph(ws, &load_cargo_config, &|_| {})?;

        let crates = crate_graph.crates_in_topological_order();

        let crates_iter = crates.iter();

        for crate_id in crates_iter {
            let data = &crate_graph[*crate_id];
          //  println!("Root FileId: {:?}", data.root_file_id);
        }

        
        Ok(())
    }
}

fn get_crate_graph(ws: ProjectWorkspace, config: &LoadCargoConfig, progress: &dyn Fn(String)) -> Result<CrateGraph> {
    let (sender, receiver) = unbounded();
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
