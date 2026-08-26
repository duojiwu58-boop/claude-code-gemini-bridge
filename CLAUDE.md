# Efficient source-reading policy

Use an evidence-driven code-review workflow. Do not mechanically page through large source files in 100-250-line chunks.

- For broad review or architecture tasks, use no more than two consecutive high-information discovery calls across `Glob`, `Grep`, and `rg`. Select the relevant implementation surface, then immediately read source. Search again later only to close a specifically named evidence gap.
- Configure `Grep` to return matching content and line numbers, not only file names, and scope it to relevant source directories rather than documentation or planning files unless those are part of the task. Once a result locates a file or symbol, read it instead of searching the same concept through alternate patterns or path scopes.
- Read complete logical units around matched symbols. Use focused ranges for isolated functions; do not scan unrelated code merely to claim whole-file coverage.
- If continuous reading of a large file is genuinely necessary, request 800-1,200 lines per `Read` call when lines are of normal size. Reduce the range only after an actual tool-size/truncation problem.
- Never continue a file with repetitive 250-line offsets such as 1, 251, 501, and 751. Re-plan with symbol search or use a larger range.
- Batch independent searches/reads when possible. Do not reread an unchanged range, repeat equivalent Grep patterns, or spend consecutive turns only searching.
- After each short navigation batch, synthesize what is proven and what evidence is still missing, then choose the next highest-value read or search. Never stop merely because of a tool-call count; continue until every material claim is supported, and explicitly qualify any area that cannot be inspected.
- For code review, prioritize concrete correctness, compatibility, security, and regression findings. Read every line when the requested scope or an unresolved cross-cutting claim genuinely requires it; otherwise use symbols, callers, tests, and configuration to establish complete evidence efficiently.
