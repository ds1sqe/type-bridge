export interface IidBearing {
    readonly _iid: string | null;
}
export declare function defineIidSlot(instance: object): void;
export declare function setIid<T extends IidBearing>(instance: T, iid: string | null): T;
