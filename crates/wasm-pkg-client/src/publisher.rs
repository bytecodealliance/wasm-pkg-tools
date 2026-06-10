use std::collections::{BTreeSet, HashMap};

use petgraph::{
    acyclic::Acyclic,
    graph::{DiGraph, NodeIndex},
};
use wasm_pkg_common::registry::{DependencyGraph, DependencyOf};

use crate::{PackageRef, PublishingSource, Version};

#[async_trait::async_trait]
pub trait PackagePublisher: Send + Sync {
    /// Publishes the data to the registry. The given data should be a valid wasm component and can
    /// be anything that implements [`AsyncRead`](tokio::io::AsyncRead) and
    /// [`AsyncSeek`](tokio::io::AsyncSeek).
    async fn publish(
        &self,
        package: &PackageRef,
        version: &Version,
        data: PublishingSource,
        dry_run: bool,
    ) -> Result<(), crate::Error>;
}

/// State for tracking dependencies during upload.
struct PublishPlan {
    /// Graph of publishable packages where the edges are `(dependency -DependencyOf->) dependent)`
    dependents: DependencyGraph<PackageRef>,
    /// Mapping [`PackageRef`]s to the respective index inside the dependency graph.
    // TODO look at using cargo's `InternedString` type for `PackageRef`:
    // https://docs.rs/cargo/latest/cargo/util/interning/struct.InternedString.html
    indices: HashMap<PackageRef, NodeIndex>,
}

impl PublishPlan {
    /// Given a package dependency graph, creates a `PublishPlan` for tracking state.
    fn new(graph: &DependencyGraph<PackageRef>) -> Self {
        let mut dependents = graph.clone().into_inner();
        dependents.reverse();
        // graph was already found to be acyclic
        let dependents = DependencyGraph::try_from(dependents).unwrap();

        let indices: HashMap<_, _> = dependents
            .nodes_iter()
            .map(|id| (dependents[id].clone(), id))
            .collect();

        Self {
            dependents,
            indices,
        }
    }

    fn iter<'a>(&'a self) -> impl Iterator<Item = &'a PackageRef> + 'a {
        self.indices.iter().map(|(pkg, _)| pkg)
    }

    fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    fn len(&self) -> usize {
        self.indices.len()
    }

    /// Returns the set of packages that are ready for publishing (i.e. have no outstanding dependencies).
    ///
    /// These will not be returned in future calls.
    fn take_ready(&mut self) -> BTreeSet<PackageRef> {
        self.dependents
            .nodes_iter()
            // there are no dependents on `self.dendents[id]`
            .filter(|id| self.dependents.neighbors(*id).count() == 0)
            .map(|id| {
                let pkg = &self.dependents[id];
                self.indices.remove(&pkg);
                pkg.clone()
            })
            .collect()
    }

    /// Packages confirmed to be available in the registry, potentially allowing additional
    /// packages to be "ready".
    fn mark_confirmed(&mut self, published: impl IntoIterator<Item = PackageRef>) {
        for pkg in published {
            let id = self
                .indices
                .remove(&pkg)
                .expect("PackageRef has no associated index");
            self.dependents
                .remove_node(id)
                .expect("index has no associated PackageRef");
        }
    }
}

/// Format a collection of packages as a list
///
/// e.g. "foo:a@0.1.0, bar:b@0.2.0, and baz:c@0.3.0".
///
/// Note: the final separator (e.g. "and" in the previous example) can be chosen.
fn package_list(pkgs: impl IntoIterator<Item = PackageRef>, final_sep: &str) -> String {
    let mut names: Vec<_> = pkgs.into_iter().map(|pkg| pkg.to_string()).collect();
    names.sort();

    match &names[..] {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} {final_sep} {b}"),
        [names @ .., last] => {
            format!("{}, {final_sep} {last}", names.join(", "))
        }
    }
}
