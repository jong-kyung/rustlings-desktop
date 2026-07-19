// @vitest-environment happy-dom

import tauriConfigSource from "../../src-tauri/tauri.conf.json?raw";
import viteConfigSource from "../../vite.config.ts?raw";
import appSource from "../App.vue?raw";
import exerciseSidebarSource from "./ExerciseSidebar.vue?raw";
import LessonPanel from "./LessonPanel.vue";
import lessonPanelSource from "./LessonPanel.vue?raw";
import runPanelSource from "./RunPanel.vue?raw";
import toolchainGateSource from "./ToolchainGate.vue?raw";
import UApp from "@nuxt/ui/components/App.vue";
import UButton from "@nuxt/ui/components/Button.vue";
import UModal from "@nuxt/ui/components/Modal.vue";
import ui from "@nuxt/ui/vue-plugin";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { createApp, h, nextTick, reactive, type App as VueApp } from "vue";

const mountedApps: VueApp[] = [];

async function settleOverlay() {
  await nextTick();
  await new Promise((resolve) => setTimeout(resolve));
}

afterEach(() => {
  for (const app of mountedApps.splice(0)) app.unmount();
  document.body.replaceChildren();
});

describe("frontend foundation", () => {
  it("resolves the standalone Nuxt UI package exports", () => {
    expect(import.meta.resolve("@nuxt/ui/vite")).toContain("@nuxt/ui/dist/vite.mjs");
    expect(typeof ui.install).toBe("function");
  });

  it("keeps the BigInt-capable WebKit and macOS bundle floors aligned", () => {
    expect(viteConfigSource).toContain('? "chrome105" : "safari14"');
    expect(JSON.parse(tauriConfigSource).bundle.macOS.minimumSystemVersion).toBe("11.0");
  });

  it("flushes dirty source before Tauri closes the window", () => {
    expect(appSource).toContain("onCloseRequested");
    expect(appSource).toContain("event.preventDefault()");
    expect(appSource).toContain("await session.flushSaves()");
  });

  it("labels the four major workspace regions with level-two headings", () => {
    expect(appSource).toMatch(/<section[^>]*aria-labelledby="code-title"/s);
    expect(appSource).toContain('<h2 id="code-title"');
    expect(exerciseSidebarSource).toMatch(/<aside[^>]*aria-labelledby="exercises-title"/s);
    expect(exerciseSidebarSource).toContain('<h2 id="exercises-title"');
    expect(lessonPanelSource).toMatch(/<aside[^>]*aria-labelledby="lesson-title"/s);
    expect(lessonPanelSource).toContain('<h2 id="lesson-title"');
    expect(runPanelSource).toMatch(/<section[^>]*aria-labelledby="run-panel-title"/s);
    expect(runPanelSource).toContain('<h2 id="run-panel-title"');
    expect(toolchainGateSource).toContain('<h3 id="toolchain-title"');
  });

  it("offers solution review only after completion", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const props = reactive({
      readme: "lesson",
      hint: undefined,
      solutionAvailable: false,
      revealingSolution: false,
      disabled: false,
    });
    let reveals = 0;
    const app = createApp({
      setup: () => () => h(LessonPanel, { ...props, onRevealSolution: () => (reveals += 1) }),
    }).use(ui);
    mountedApps.push(app);
    app.mount(host);

    expect(document.body.textContent).not.toContain("Review solution");
    props.solutionAvailable = true;
    await nextTick();
    const review = [...document.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => button.textContent?.trim() === "Review solution",
    );
    review?.click();

    expect(review).toBeDefined();
    expect(reveals).toBe(1);
  });

  it("renders solution comparison as escaped keyboard-scrollable code", () => {
    expect(appSource).toContain('v-model:open="solutionReviewOpen"');
    expect(appSource.match(/<pre[^>]*tabindex="0"/g)).toHaveLength(2);
    expect(appSource).toContain("Your solution");
    expect(appSource).toContain("Reference solution");
  });

  it("provides one escaped overlay and restores trigger focus", async () => {
    const host = document.createElement("div");
    document.body.append(host);

    const plainTextFixture = '<script>alert("escaped")<' + "/script>";
    const app = createApp({
      setup: () => () =>
        h(UApp, { toaster: null }, () =>
          h(
            UModal,
            { title: "Local text preview", close: false, transition: false },
            {
              default: () => h(UButton, { type: "button", label: "Test overlay" }),
              body: () => h("p", { textContent: plainTextFixture }),
            },
          ),
        ),
    }).use(ui);
    mountedApps.push(app);
    app.mount(host);

    const trigger = document.querySelector<HTMLButtonElement>("button");
    expect(trigger).not.toBeNull();
    trigger?.focus();
    trigger?.click();
    await settleOverlay();

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog?.textContent).toContain('<script>alert("escaped")</script>');
    expect(dialog?.querySelector("script")).toBeNull();

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settleOverlay();

    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });
});
