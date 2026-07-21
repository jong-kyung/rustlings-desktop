// @vitest-environment happy-dom

import ui from "@nuxt/ui/vue-plugin";
import "virtual:nuxt-icon-bundle/register";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { createApp, h, nextTick, reactive, type App as VueApp } from "vue";
import type {
  RunResponse,
  RunTarget,
  ValidationOutcome,
  ValidationResult,
} from "../types/learning";
import RunPanel from "./RunPanel.vue";

const mountedApps: VueApp[] = [];

function validation(outcome: ValidationOutcome): ValidationResult {
  return {
    exercise_id: "intro1",
    source_digest: "digest-0",
    outcome,
    stages: [],
    diagnostics: [],
  };
}

function result(outcome: ValidationOutcome, overrides: Partial<RunResponse> = {}): RunResponse {
  return {
    runId: "run-1",
    target: { kind: "learner", exerciseId: "intro1" },
    revision: 0,
    stale: false,
    validation: validation(outcome),
    finalRecheck: [],
    snapshot: {
      selected: "intro1",
      source: "original",
      sourceDigest: "digest-0",
      readme: "readme",
      exercises: [
        {
          id: "intro1",
          sourcePath: "exercises/00_intro/intro1.rs",
          solutionPath: "solutions/00_intro/intro1.rs",
          solutionAvailable: false,
          status: "current",
          revision: 0,
        },
      ],
      activeRun: null,
      curriculumComplete: false,
      preflight: { ready: true, message: null, rustcVersion: "1.88.0" },
    },
    ...overrides,
  };
}

async function settle() {
  await nextTick();
  await Promise.resolve();
}

afterEach(() => {
  for (const app of mountedApps.splice(0)) app.unmount();
  document.body.replaceChildren();
});

describe("RunPanel", () => {
  it("renders outcome precedence and disables Run and Cancel from explicit capabilities", async () => {
    const props = reactive({
      result: undefined as RunResponse | undefined,
      target: undefined as RunTarget | undefined,
      running: false,
      canCancel: false,
      cancelling: false,
      runDisabled: false,
      error: undefined as string | undefined,
    });
    const host = document.createElement("div");
    document.body.append(host);
    const app = createApp({ setup: () => () => h(RunPanel, props) }).use(ui);
    mountedApps.push(app);
    app.mount(host);
    await settle();

    const status = () => host.querySelector('[role="status"]')?.textContent?.trim();
    const button = (label: string) =>
      [...host.querySelectorAll<HTMLButtonElement>("button")].find((item) =>
        item.textContent?.includes(label),
      )!;

    expect(status()).toBe("Ready to run.");
    expect(button("Run").disabled).toBe(false);
    expect(button("Cancel").disabled).toBe(true);

    const outcomes: Array<[ValidationOutcome, string]> = [
      [{ status: "passed" }, "Learner code: Passed."],
      [{ status: "learner_failure", stage: "test" }, "Learner code: Needs another try (test)."],
      [{ status: "cancelled" }, "Learner code: Cancelled."],
      [{ status: "timed_out" }, "Learner code: Timed out."],
      [{ status: "output_limit" }, "Learner code: Output limit reached."],
      [
        { status: "operational_failure", kind: "process", message: "runner unavailable" },
        "Learner code: Validation unavailable: runner unavailable",
      ],
    ];
    for (const [outcome, label] of outcomes) {
      props.result = result(outcome);
      await settle();
      expect(status()).toBe(label);
    }

    props.result = result(
      { status: "passed" },
      {
        target: {
          kind: "solution",
          exerciseId: "intro1",
          path: "solutions/00_intro/intro1.rs",
        },
        validation: {
          ...validation({ status: "passed" }),
          stages: [
            {
              stage: "build",
              success: true,
              stdout: "checked solution",
              stderr: "",
              output_truncated: false,
            },
          ],
        },
      },
    );
    await settle();
    expect(status()).toBe("Solution code: Passed.");
    expect(host.textContent).toContain("Solution code · intro1 · build");

    props.result = result(
      { status: "passed" },
      { finalRecheck: [validation({ status: "learner_failure", stage: "clippy" })] },
    );
    await settle();
    expect(status()).toBe("Final recheck: Needs another try (clippy).");

    props.result = result(
      { status: "operational_failure", kind: "storage", message: "progress not saved" },
      { finalRecheck: [validation({ status: "passed" })] },
    );
    await settle();
    expect(status()).toBe("Learner code: Validation unavailable: progress not saved");

    props.result = result({ status: "cancelled" });
    props.result.snapshot.curriculumComplete = true;
    await settle();
    expect(status()).toBe("Learner code: Cancelled.");

    props.result = result(
      { status: "learner_failure", stage: "build" },
      {
        validation: {
          ...validation({ status: "learner_failure", stage: "build" }),
          diagnostics: [
            {
              stage: "build",
              severity: "error",
              message: "learner-only range",
              code: null,
              range: {
                start_line_number: 1,
                start_column: 1,
                end_line_number: 1,
                end_column: 2,
              },
              source_digest: "digest-0",
            },
          ],
        },
      },
    );
    props.target = {
      kind: "solution",
      exerciseId: "intro1",
      path: "solutions/00_intro/intro1.rs",
    };
    await settle();
    expect(button("learner-only range")).toBeUndefined();
    expect(host.textContent).toContain("learner-only range");

    props.target = { kind: "learner", exerciseId: "intro1" };
    await settle();
    expect(button("learner-only range")).toBeDefined();
    expect(button("learner-only range").disabled).toBe(false);

    props.target = undefined;
    props.result = result({ status: "passed" }, { stale: true });
    props.result.snapshot.curriculumComplete = true;
    props.error = "disk error";
    await settle();
    expect(status()).toBe("Error: disk error");

    props.running = true;
    await settle();
    expect(status()).toBe("Validation running…");
    expect(button("Run").disabled).toBe(true);
    expect(button("Cancel").disabled).toBe(true);

    props.canCancel = true;
    await settle();
    expect(button("Cancel").disabled).toBe(false);

    props.cancelling = true;
    await settle();
    expect(status()).toBe("Cancelling validation…");
    expect(button("Cancel").disabled).toBe(true);

    props.running = false;
    props.cancelling = false;
    props.error = undefined;
    await settle();
    expect(status()).toBe("Stale result — source changed; progress and markers were not updated.");

    props.result.stale = false;
    await settle();
    expect(status()).toBe("All exercises completed.");

    props.runDisabled = true;
    await settle();
    expect(button("Run").disabled).toBe(true);
  });

  it("shows a play icon on the Run button", async () => {
    const props = reactive({
      result: undefined as RunResponse | undefined,
      target: undefined as RunTarget | undefined,
      running: false,
      canCancel: false,
      cancelling: false,
      runDisabled: false,
      error: undefined as string | undefined,
    });
    const host = document.createElement("div");
    document.body.append(host);
    const app = createApp({ setup: () => () => h(RunPanel, props) }).use(ui);
    mountedApps.push(app);
    app.mount(host);
    await settle();

    const run = [...host.querySelectorAll<HTMLButtonElement>("button")].find((item) =>
      item.textContent?.includes("Run"),
    )!;
    expect(run.querySelector(".iconify--lucide")).not.toBeNull();
  });

  it("opens a verdict dialog for fresh learner pass/fail results only", async () => {
    const props = reactive({
      result: undefined as RunResponse | undefined,
      target: undefined as RunTarget | undefined,
      running: false,
      canCancel: false,
      cancelling: false,
      runDisabled: false,
      error: undefined as string | undefined,
    });
    const host = document.createElement("div");
    document.body.append(host);
    const app = createApp({ setup: () => () => h(RunPanel, props) }).use(ui);
    mountedApps.push(app);
    app.mount(host);
    await settle();

    const dialog = () => document.querySelector('[role="dialog"]');
    const dialogButton = (label: string) =>
      [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')].find((item) =>
        item.textContent?.includes(label),
      );

    const silentOutcomes: ValidationOutcome[] = [
      { status: "cancelled" },
      { status: "timed_out" },
      { status: "output_limit" },
      { status: "operational_failure", kind: "process", message: "runner unavailable" },
    ];
    for (const [index, outcome] of silentOutcomes.entries()) {
      props.result = result(outcome, { runId: `silent-${index}` });
      await settle();
      expect(dialog()).toBeNull();
    }

    props.result = result({ status: "passed" }, { runId: "stale-1", stale: true });
    await settle();
    expect(dialog()).toBeNull();

    props.result = result(
      { status: "passed" },
      {
        runId: "solution-1",
        target: { kind: "solution", exerciseId: "intro1", path: "solutions/00_intro/intro1.rs" },
      },
    );
    await settle();
    expect(dialog()).toBeNull();

    props.result = result({ status: "passed" }, { runId: "run-pass" });
    await settle();
    expect(dialog()?.textContent).toContain("Exercise passed");
    expect(dialog()?.textContent).toContain("intro1 passed.");
    expect(host.querySelector('[role="status"]')?.textContent?.trim()).toBe(
      "Learner code: Passed.",
    );

    dialogButton("Continue")?.click();
    await settle();
    expect(dialog()).toBeNull();

    // A retained result resurfacing as a new object (same runId) must not re-open.
    props.result = result({ status: "passed" }, { runId: "run-pass" });
    await settle();
    expect(dialog()).toBeNull();

    props.result = result({ status: "learner_failure", stage: "build" }, { runId: "run-fail" });
    await settle();
    expect(dialog()?.textContent).toContain("Not yet");
    expect(dialog()?.textContent).toContain("Needs another try (build).");
    dialogButton("Try again")?.click();
    await settle();
    expect(dialog()).toBeNull();

    // Dialog verdict follows the same validation as the status line: a passing
    // primary run whose final recheck fails announces failure, not success.
    props.result = result(
      { status: "passed" },
      {
        runId: "run-recheck",
        finalRecheck: [validation({ status: "learner_failure", stage: "clippy" })],
      },
    );
    await settle();
    expect(dialog()?.textContent).toContain("Not yet");
    expect(dialog()?.textContent).toContain("Final recheck of intro1: Needs another try (clippy).");
    dialogButton("Try again")?.click();
    await settle();

    props.result = result({ status: "passed" }, { runId: "run-complete" });
    props.result.snapshot.curriculumComplete = true;
    await settle();
    expect(dialog()?.textContent).toContain("All exercises completed.");
  });
});
