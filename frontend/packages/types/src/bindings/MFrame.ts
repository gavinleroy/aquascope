// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { CharRange } from "./CharRange";
import type { MLocal } from "./MLocal";

export interface MFrame<L> { name: string, body_span: CharRange, location: L, locals: Array<MLocal>, }