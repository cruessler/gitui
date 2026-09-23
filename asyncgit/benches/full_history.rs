use anyhow::{ensure, Context, Result};
use asyncgit::{
	sync::{get_branches_info, get_head, CommitId, RepoPath},
	AsyncGitNotification, AsyncLog, FetchStatus,
};
use criterion::{BatchSize, Criterion, Throughput};
use crossbeam_channel::{unbounded, Receiver};
use git2::{Repository, RepositoryOpenFlags};
use std::{
	collections::HashSet,
	env,
	hint::black_box,
	path::{Path, PathBuf},
	time::{Duration, Instant},
};

// 1200 is the value used by the main application. See `src/popups/file_revlog.rs`.
const WINDOW_SIZE: usize = 1200;

struct Fixture {
	repo: RepoPath,
	head: CommitId,
	commit_count: usize,
	branch_tips: HashSet<CommitId>,
	stashes: HashSet<CommitId>,
	head_id: Option<CommitId>,
	timeout: Duration,
}

impl Fixture {
	fn new(repo: RepoPath, timeout: Duration) -> Result<Self> {
		ensure!(
			!timeout.is_zero(),
			"benchmark timeout must be positive"
		);
		let head = get_head(&repo)
			.context("repository must have a readable HEAD")?;

		// This is a very defensive way of counting the number of commits in this benchmark. The
		// benchmarked code uses `gix` under the hood while the count is calculated using `libgit2`.
		// Any mismatch between the two implementations would make the benchmark fail later.
		let repository = Repository::open_ext(
			repo.gitpath(),
			RepositoryOpenFlags::FROM_ENV,
			None::<&Path>,
		)?;
		let mut walk = repository.revwalk()?;
		walk.push_head()?;
		let commit_count =
			walk.try_fold(0, |count, id| id.map(|_| count + 1))?;
		ensure!(
			commit_count > 0,
			"benchmark requires a nonempty history"
		);

		let local = get_branches_info(&repo, true)?;
		let remote = get_branches_info(&repo, false)?;
		let head_id = local.iter().find_map(|branch| {
			branch
				.local_details()
				.is_some_and(|details| details.is_head)
				.then_some(branch.top_commit)
		});
		let branch_tips = local
			.iter()
			.chain(&remote)
			.map(|branch| branch.top_commit)
			.collect();

		Ok(Self {
			repo,
			head,
			commit_count,
			branch_tips,
			stashes: HashSet::new(),
			head_id,
			timeout,
		})
	}

	fn iteration(&self, graph_enabled: bool) -> Result<Iteration> {
		ensure!(get_head(&self.repo)? == self.head, "HEAD changed during benchmark; keep the repository unchanged");
		let (sender, receiver) = unbounded();
		Ok(Iteration {
			git_log: AsyncLog::new(self.repo.clone(), &sender, None),
			receiver,
			graph_enabled,
		})
	}
}

struct Iteration {
	git_log: AsyncLog,
	receiver: Receiver<AsyncGitNotification>,
	graph_enabled: bool,
}

struct Coverage {
	commits: usize,
	graph_rows: usize,
}

impl Iteration {
	fn run(&self, fixture: &Fixture) -> Result<Coverage> {
		let deadline = Instant::now()
			.checked_add(fixture.timeout)
			.context("benchmark timeout is too large")?;
		ensure!(
			self.git_log.fetch()? == FetchStatus::Started,
			"expected a fresh history fetch"
		);

		while self.git_log.is_pending() {
			self.receiver
				.recv_deadline(deadline)
				.context("timed out waiting for history fetch")?;
		}
		let history = self.git_log.extract_items()?;
		ensure!(history.len() == fixture.commit_count, "history walk did not return the expected number of commits");

		let mut coverage = Coverage {
			commits: 0,
			graph_rows: 0,
		};
		for commits in history.chunks(WINDOW_SIZE) {
			ensure!(
				Instant::now() < deadline,
				"full-history benchmark timed out"
			);
			if self.graph_enabled {
				let rows = self
					.git_log
					.get_graph_rows(
						commits,
						coverage.commits,
						&fixture.branch_tips,
						&fixture.stashes,
						fixture.head_id.as_ref(),
					)
					.context("graph generation failed")?;
				ensure!(
					rows.len() == commits.len(),
					"graph generation returned incomplete rows"
				);
				coverage.graph_rows += rows.len();
				black_box(rows);
			} else {
				black_box(commits);
			}
			coverage.commits += commits.len();
		}
		ensure!(
			coverage.commits == fixture.commit_count,
			"incorrect history coverage"
		);
		ensure!(
			coverage.graph_rows
				== if self.graph_enabled {
					fixture.commit_count
				} else {
					0
				},
			"incorrect graph coverage"
		);
		Ok(coverage)
	}
}

fn main() -> Result<()> {
	let repo =
		PathBuf::from(env::var_os("GITUI_BENCH_REPO").context(
			"set GITUI_BENCH_REPO to the repository to benchmark",
		)?);
	let timeout = match env::var("GITUI_BENCH_TIMEOUT_SECS") {
		Ok(value) => value.parse::<u64>()?,
		Err(env::VarError::NotPresent) => 1200,
		Err(err) => return Err(err.into()),
	};
	ensure!(timeout > 0, "GITUI_BENCH_TIMEOUT_SECS must be positive");

	// The main application also uses 4 threads. See `gitui::set_panic_handler()`.
	rayon_core::ThreadPoolBuilder::new()
		.num_threads(4)
		.build_global()?;
	let fixture =
		Fixture::new(repo.into(), Duration::from_secs(timeout))?;
	let mut criterion =
		Criterion::default().sample_size(10).configure_from_args();
	let mut group = criterion.benchmark_group("full_history");
	group.throughput(Throughput::Elements(
		fixture.commit_count.try_into()?,
	));
	for (name, enabled) in
		[("without_graph", false), ("with_graph", true)]
	{
		group.bench_function(name, |b| {
			b.iter_batched_ref(
				|| {
					fixture.iteration(enabled).expect(
						"failed to initialize history benchmark",
					)
				},
				|iteration| {
					black_box(
						iteration
							.run(&fixture)
							.expect("full-history benchmark failed"),
					);
				},
				BatchSize::PerIteration,
			);
		});
	}
	group.finish();
	criterion.final_summary();
	Ok(())
}
