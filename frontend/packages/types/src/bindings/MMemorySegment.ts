// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.

export type MMemorySegment =
  | { type: "Stack"; value: { frame: number; local: string } }
  | { type: "Heap"; value: { index: number } };