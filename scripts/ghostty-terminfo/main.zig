const std = @import("std");
const terminfo = @import("terminfo");

pub fn main(init: std.process.Init) !void {
    var buffer: [4096]u8 = undefined;
    var stdout = std.Io.File.stdout().writerStreaming(init.io, &buffer);
    try terminfo.ghostty.encode(&stdout.interface);
    try stdout.end();
}
