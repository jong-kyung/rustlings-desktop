// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import tauriConfigSource from "../../src-tauri/tauri.conf.json?raw";
import viteConfigSource from "../../vite.config.ts?raw";
import appSource from "../App.vue?raw";
import exerciseSidebarSource from "./ExerciseSidebar.vue?raw";
import lessonPanelSource from "./LessonPanel.vue?raw";
import runPanelSource from "./RunPanel.vue?raw";
import toolchainGateSource from "./ToolchainGate.vue?raw";
import UApp from "@nuxt/ui/components/App.vue";
import UButton from "@nuxt/ui/components/Button.vue";
import UModal from "@nuxt/ui/components/Modal.vue";
import ui from "@nuxt/ui/vue-plugin";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { createApp, h, nextTick, type App as VueApp } from "vue";

const mountedApps: VueApp[] = [];
const styleSource = readFileSync(resolve(process.cwd(), "src/style.css"), "utf8");

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
    expect(exerciseSidebarSource).toContain(">Rustlings</h2>");
    expect(lessonPanelSource).toMatch(/<aside[^>]*aria-labelledby="lesson-title"/s);
    expect(lessonPanelSource).toContain('<h2 id="lesson-title"');
    expect(runPanelSource).toMatch(/<section[^>]*aria-labelledby="run-panel-title"/s);
    expect(runPanelSource).toContain('<h2 id="run-panel-title"');
    expect(toolchainGateSource).toContain('<h3 id="toolchain-title"');
  });

  it("opens completed solutions from the tree instead of a comparison overlay", () => {
    expect(exerciseSidebarSource).toContain("solutionAvailable");
    expect(exerciseSidebarSource).toContain("emit('selectSolution', item.exercise.id)");
    expect(appSource).toContain('@select-solution="session.revealSolution"');
    expect(appSource).not.toContain("solutionReviewOpen");
  });

  it("keeps exercise scrolling between the fixed search header and save footer", () => {
    expect(exerciseSidebarSource).toContain('type="search"');
    expect(exerciseSidebarSource).toContain('class="exercise-tree-scroll p-2"');
    expect(exerciseSidebarSource).toContain("<footer");
    expect(appSource).toContain(':selected-solution="session.solution.value?.exerciseId"');
  });

  it("activates the custom-property sidebar layout at 800px without widening content columns", () => {
    expect(styleSource).toMatch(
      /@media \(min-width: 50rem\)[\s\S]*grid-template-columns:\s*var\(--sidebar-width, 272px\)/,
    );
    expect(styleSource).toMatch(
      /@media \(min-width: 64rem\)[\s\S]*grid-template-columns:\s*minmax\(20rem, 2fr\) minmax\(16rem, 1fr\)/,
    );
    expect(styleSource).not.toContain("min-block-size: 38rem");
    expect(appSource).toContain('role="separator"');

    const windowConfig = JSON.parse(tauriConfigSource).app.windows[0];
    expect(windowConfig.width).toBe(800);
    expect(windowConfig.height).toBe(600);
    expect(windowConfig.minWidth).toBe(800);
    expect(windowConfig.minHeight).toBe(600);
  });

  it("renders solutions in the read-only Monaco workspace with their matching lesson", () => {
    expect(appSource).toContain(':read-only="session.viewingSolution.value"');
    expect(appSource).toContain("Read-only Rust solution editor");
    expect(appSource).toContain("Current solution");
    expect(appSource).toContain(':readme="readme"');
    expect(appSource).toContain(':show-hint="!session.viewingSolution.value"');
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
