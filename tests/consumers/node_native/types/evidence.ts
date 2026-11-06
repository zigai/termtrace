import { Events, Output, type ProjectedEventPage, type ByteOutputPage } from "@termtrace/core";

const events = new Events(new Uint8Array([47, 116, 109, 112]));
const output = new Output(new Uint8Array([47, 116, 109, 112]));
function bytes(page: ByteOutputPage): bigint {
    const exact: Uint8Array = page.bytes;
    void exact;
    return page.cursor.byte_offset;
}
function sequence(page: ProjectedEventPage): bigint {
    const event = page.events[0].event;
    if (event?.type === "output") {
        const exact: Uint8Array = event.data;
        void exact;
    }
    return page.cursor.next_event_seq;
}
void output.page().then(bytes);
void events.page(null, 100, false, ["output"]).then(sequence);
void events.follow(9007199254740993n);
void output.waitFor(new Uint8Array([0, 255]), 1250);
// @ts-expect-error wide offsets are not numbers
void output.page({ byte_offset: 1 });
// @ts-expect-error follow positions are exact bigints
void events.follow(1);
// @ts-expect-error byte waits require native byte buffers
void output.waitFor("READY");
void events.page(null, 100, false, ["output"]);
