import ui from "@nuxt/ui/vue-plugin";
import "virtual:nuxt-icon-bundle/register";
import { createApp } from "vue";
import App from "./App.vue";
import "./style.css";

createApp(App).use(ui).mount("#app");
