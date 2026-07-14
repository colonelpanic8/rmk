import { RynkClient, type LedIndicator } from "./pkg/rynk_wasm.js";

declare const client: RynkClient;
const indicator: Promise<LedIndicator> = client.get_led_indicator();
void indicator;
