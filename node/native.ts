import { resolve } from "node:path";

import type * as NativeBinding from "./index.js";

const binding = require(resolve(__dirname, "..", "index.js")) as typeof NativeBinding;

export const { MboDecoder, MboSweepDetector, decodeStats } = binding;
export type { JsSweepConfig, JsSweepEvent } from "./index.js";
