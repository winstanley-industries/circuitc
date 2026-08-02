# GitHub exact-head closeout

## Contents

- [Snapshot the PR](#snapshot-the-pr)
- [Watch checks](#watch-checks)
- [Read and resolve threads](#read-and-resolve-threads)
- [Interpret feedback without looping](#interpret-feedback-without-looping)
- [Merge with expected-head protection](#merge-with-expected-head-protection)

Prefer the installed GitHub skills for publication, CI diagnosis, and review-thread work. Use authenticated `gh` when connector coverage is insufficient.

## Snapshot the PR

Capture the exact head before interpreting checks or reviews:

```sh
gh pr view NUMBER --repo OWNER/REPO \
  --json number,url,state,isDraft,mergeable,reviewDecision,headRefName,headRefOid,baseRefName,statusCheckRollup
gh pr checks NUMBER --repo OWNER/REPO
```

Fetch thread state through `github:gh-address-comments` or the read-only GraphQL query below. Repeat the complete snapshot after every push and before merge. Do not combine a current head with an earlier check or verdict.

## Watch checks

```sh
gh pr checks NUMBER --repo OWNER/REPO --watch --interval 10
```

Keep user-facing updates flowing during long waits. Inspect failing Actions logs with the `github:gh-fix-ci` workflow. A successful manual run does not replace a required PR-event check.

## Read and resolve threads

Use `github:gh-address-comments` first because flat issue comments do not contain reliable thread-resolution state. When it is unavailable, fetch exact-head thread state with this read-only operation:

```graphql
query ReviewThreads(
  $owner: String!
  $name: String!
  $number: Int!
  $after: String
) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      headRefOid
      reviewThreads(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          originalLine
          comments(first: 100) {
            pageInfo { hasNextPage endCursor }
            nodes { id author { login } body url createdAt }
          }
        }
      }
    }
  }
}
```

Submit it with authenticated `gh api graphql`, using raw `-f` fields for `owner` and `name`, and typed `-F` for `number`. Omit `after` on the first request so the nullable variable is unset; pass the cursor with `-f after=END_CURSOR` only on subsequent pages. Continue with each review-thread `endCursor` until `hasNextPage` is false. Require the same non-empty `headRefOid` on every page; restart if it moves. Count every node where `isResolved` is false, preserving `isOutdated` in the report. This operation is a query and must not be combined with reply or resolution mutations.

If an unresolved thread's comment connection has another page, fetch its remaining context before classification:

```graphql
query ThreadComments($threadId: ID!, $after: String) {
  node(id: $threadId) {
    ... on PullRequestReviewThread {
      id
      isResolved
      comments(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes { id author { login } body url createdAt }
      }
    }
  }
}
```

Pass `threadId` with raw `-f`, omit `after` on the first request, and pass each later cursor with raw `-f`. Recheck the PR head before and after paging nested comments and restart the whole snapshot if it moved.

When a thread-aware reply tool is unavailable, post the factual reply with GraphQL:

```graphql
mutation ReplyToReviewThread($threadId: ID!, $body: String!) {
  addPullRequestReviewThreadReply(
    input: { pullRequestReviewThreadId: $threadId, body: $body }
  ) {
    comment { id url }
  }
}
```

Write the reply body to a task-scoped file so shell quoting cannot alter the evidence, then invoke the mutation with authenticated `gh`. Use typed `-F body=@FILE` to read the file; raw `-f body=@FILE` would post the literal filename.

```sh
reply_file=.agent-scratch/pr-review-reply.md
gh api graphql \
  -f 'query=mutation ReplyToReviewThread($threadId: ID!, $body: String!) { addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $threadId, body: $body }) { comment { id url } } }' \
  -f threadId=THREAD_NODE_ID \
  -F body=@"$reply_file"
```

When resolution support is missing, resolve individual addressed threads with GraphQL:

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
  -f threadId=THREAD_NODE_ID
```

Post the factual reply before resolution using the available thread-aware GitHub tool. Include the fixing commit and the narrow/full validation that confirmed it. Reply and resolution are separate writes; verify both.

After each mutation batch, run the read-only thread query again and require zero unresolved current and outdated threads before merge. A partially addressed or disputed thread blocks merge until fixed or conclusively disposed and resolved; a recorded note alone does not waive the gate.

If batching GraphQL aliases, keep batches at 24 threads or fewer and inspect every result. Large alias batches can exceed GitHub's GraphQL complexity limit. Never use a blanket resolve-all mutation without first classifying every unresolved thread.

## Interpret feedback without looping

- Fix contract, correctness, security, and missing-regression-test blockers on the owning layer.
- Move a valid cross-contract issue to the correct upper layer, link the destination in the reply, and obtain acceptance before resolving the current thread.
- Answer stale or incorrect feedback with repository evidence.
- Avoid changing an approved exact head for ordinary nits; record and link a follow-up, then obtain acceptance before resolving the current thread.
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
