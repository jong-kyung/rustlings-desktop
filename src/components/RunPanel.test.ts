// @vitest-environment happy-dom

import ui from "@nuxt/ui/vue-plugin";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { createApp, h, nextTick, reactive, type App as VueApp } from "vue";
import type { RunResponse, ValidationOutcome, ValidationResult } from "../types/learning";
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
    revision: 0,
    stale: false,
    validation: validation(outcome),
    finalRecheck: [],
    snapshot: {
      selected: "intro1",
      source: "original",
      sourceDigest: "digest-0",
      readme: "readme",
      exercises: [{ id: "intro1", status: "current", revision: 0 }],
      activeRunId: null,
      sliceComplete: false,
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
      [{ status: "passed" }, "Passed."],
      [{ status: "learner_failure", stage: "test" }, "Needs another try (test)."],
      [{ status: "cancelled" }, "Cancelled."],
      [{ status: "timed_out" }, "Timed out."],
      [{ status: "output_limit" }, "Output limit reached."],
      [
        { status: "operational_failure", kind: "process", message: "runner unavailable" },
        "Validation unavailable: runner unavailable",
      ],
    ];
    for (const [outcome, label] of outcomes) {
      props.result = result(outcome);
      await settle();
      expect(status()).toBe(label);
    }

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
    expect(status()).toBe("Validation unavailable: progress not saved");

    props.result = result({ status: "cancelled" });
    props.result.snapshot.sliceComplete = true;
    await settle();
    expect(status()).toBe("Cancelled.");

    props.result = result({ status: "passed" }, { stale: true });
    props.result.snapshot.sliceComplete = true;
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
});
