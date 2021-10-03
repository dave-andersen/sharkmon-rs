# sharkmon-rs
Small monitor + embedded web server to monitor the Electro Industries Shark 100S

The EIG Shark 100S (and others in this family) have an integrated Ethernet/Wifi
TCP bridge for their Modbus access, but do not have an easy-to-view web server
to surface the data available.

This small rust program does what you'd think: Connects to the Shark, reads its
modbus information once per second, and makes the averaged data on watts,
volts, and frequency available as a simple web page suitable for
displaying on an iPad as a power monitor.

Visit http://localhost:8081/ to see the page, or
http://localhost:8081/power to see a JSON summary of the power data.

![screen shot of sharkmon web page](https://github.com/dave-andersen/sharkmon-rs/blob/main/sharkmon.png?raw=true)

Use:

```
   cargo build --release
   ./target/release/sharkmon 192.168.1.100:502
```

If you just want to have the output logged to console or to a file, use:
```
   sharkmon -n <meter>
```

Example output:
```
{"watts":15944.473,"volts":237.37653,"frequency":59.980522}
{"watts":15939.086,"volts":237.3808,"frequency":59.980812}
```

Note that sharkmon expects the file "sharkmon.html" to be in the same directory from which you run it.


