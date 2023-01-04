// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { Abbreviated } from "./Abbreviated";
import type { MPath } from "./MPath";

export type MValue = { type: "Bool", value: boolean } | { type: "Char", value: string } | { type: "Uint", value: bigint } | { type: "Int", value: bigint } | { type: "Float", value: number } | { type: "Pointer", value: MPath } | { type: "Struct", value: { name: string, fields: Array<[string, MValue]>, } } | { type: "Enum", value: { name: string, variant: string, fields: Array<[string, MValue]>, } } | { type: "String", value: Abbreviated<bigint> } | { type: "Array", value: Abbreviated<MValue> } | { type: "Unallocated" };
