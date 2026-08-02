# GitHub exact-head closeout

Prefer the installed GitHub skills for publication, CI diagnosis, and review-thread work. Use authenticated `gh` when connector coverage is insufficient.

## Snapshot the PR

Capture the exact head before interpreting checks or reviews:

```sh
gh pr view NUMBER --repo OWNER/REPO \
  --json number,url,state,isDraft,mergeable,reviewDecision,headRefName,headRefOid,baseRefName,statusCheckRollup
gh pr checks NUMBER --repo OWNER/REPO
python3 scripts/pr_thread_status.py --repo OWNER/REPO --pr NUMBER
```

Resolve the script path relative to the skill directory. Repeat the snapshot after every push and before merge. Do not combine a current head with an earlier check or verdict.

## Watch checks

```sh
gh pr checks NUMBER --repo OWNER/REPO --watch --interval 10
```

Keep user-facing updates flowing during long waits. Inspect failing Actions logs with the `github:gh-fix-ci` workflow. A successful manual run does not replace a required PR-event check.

## Read and resolve threads

Use `github:gh-address-comments` first because flat issue comments do not contain reliable thread-resolution state. The bundled helper is read-only and suitable for the before/after count.

When resolution support is missing, fetch thread IDs with GraphQL and resolve individual addressed threads:

```graphql
mutation ResolveReviewThread($threadId: ID!) {
  resolveReviewThread(input: {threadId: $threadId}) {
    thread { id isResolved }
  }
}
```

Invoke it with authenticated `gh`:

```sh
gh api graphql \
  -f 'query=mutation ResolveReviewThread($threadId: ID!) { resolveReviewThread(input: {threadId: $threadId}) { thread { id isResolved } } }' \
  -F threadId=THREAD_NODE_ID
```

Post the factual reply before resolution using the available thread-aware GitHub tool. Include the fixing commit and the narrow/full validation that confirmed it. Reply and resolution are separate writes; verify both.

If batching GraphQL aliases, keep batches at 24 threads or fewer and inspect every result. Large alias batches can exceed GitHub's GraphQL complexity limit. Never use a blanket resolve-all mutation without first classifying every unresolved thread.

## Interpret feedback without looping

- Fix contract, correctness, security, and missing-regression-test blockers on the owning layer.
- Move a valid cross-contract issue to the correct upper layer.
- Answer stale or incorrect feedback with repository evidence.
- Avoid changing an approved exact head for ordinary nits; record a follow-up instead.
- After two rounds that uncover blockers in different new subsystems, split or restack before making another broad patch.

## Merge with expected-head protection

First inspect repository merge policy:

```sh
gh repo view OWNER/REPO --json mergeCommitAllowed,rebaseMergeAllowed,squashMergeAllowed
```

Then merge only the verified exact head, using the repository-approved method:

```sh
gh pr merge NUMBER --repo OWNER/REPO --squash --match-head-commit HEAD_OID
```

Add `--delete-branch` only when remote branch cleanup is authorized. Re-read the PR after any merge error. Remote merge can succeed while local branch cleanup fails. Fetch `origin/main` and verify the expected merged tree before advancing the stack.
