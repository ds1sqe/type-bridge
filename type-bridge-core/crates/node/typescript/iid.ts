export interface IidBearing {
  readonly _iid: string | null;
}

const iidSlot = "_iid";

export function defineIidSlot(instance: object): void {
  Object.defineProperty(instance, iidSlot, {
    value: null,
    enumerable: false,
    writable: true,
  });
}

export function setIid<T extends IidBearing>(instance: T, iid: string | null): T {
  Object.defineProperty(instance, iidSlot, {
    value: iid,
    enumerable: false,
    writable: true,
  });
  return instance;
}
