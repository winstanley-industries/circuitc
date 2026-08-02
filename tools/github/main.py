"""Command-line entry point for CircuitC's PR thread status helper."""

from tools.github import pr_thread_status

if __name__ == "__main__":
    raise SystemExit(pr_thread_status.main())
