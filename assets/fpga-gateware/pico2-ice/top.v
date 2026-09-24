// Default gateware for the pico2-ice FPGA loader the IDE generates.
//
// It is deliberately NOT the factory image. The FPGA's own flash already holds
// tinyVision's blink (dark, red, blue, green, 0.35 s each), and the FPGA boots
// it by itself whenever the RP2350 releases CRESET without loading anything -
// so a blink that looked like the factory one would prove nothing. This one
// tells the two apart:
//
//   RED   blinking   this design runs. It uses the FPGA's internal oscillator,
//                    so it needs nothing from the RP2350. The factory image
//                    blinks red too - it is BLUE staying dark that says this
//                    one was loaded.
//   GREEN blinking   the RP2350's clock reaches FPGA pin 35 (GP21 = GPOUT0).
//   BLUE             never lights. The factory image lights it.
//
// To rebuild it, or to put your own design in its place, see README.md.
module top (
    input  CLK,    // pin 35, driven by the RP2350's GP21
    output LED_R,  // pin 41 (RGB2), active low
    output LED_G,  // pin 39 (RGB0), active low
    output LED_B   // pin 40 (RGB1), active low
);
    // The internal 48 MHz oscillator, divided by 4: 12 MHz.
    wire hf;
    SB_HFOSC #(.CLKHF_DIV("0b10")) osc (.CLKHFPU(1'b1), .CLKHFEN(1'b1), .CLKHF(hf));

    reg [23:0] hf_cnt = 0;
    always @(posedge hf) hf_cnt <= hf_cnt + 1;   // bit 23 flips every ~0.7 s

    reg [23:0] rp_cnt = 0;
    always @(posedge CLK) rp_cnt <= rp_cnt + 1;  // every ~0.17 s at 48 MHz

    // The RGB pins are the iCE40's current sinks: only SB_RGBA_DRV drives them.
    SB_RGBA_DRV #(
        .CURRENT_MODE("0b1"),
        .RGB0_CURRENT("0b000001"),
        .RGB1_CURRENT("0b000001"),
        .RGB2_CURRENT("0b000001")
    ) rgb (
        .CURREN(1'b1),
        .RGBLEDEN(1'b1),
        .RGB0PWM(rp_cnt[23]),  // green
        .RGB1PWM(1'b0),        // blue
        .RGB2PWM(hf_cnt[23]),  // red
        .RGB0(LED_G),
        .RGB1(LED_B),
        .RGB2(LED_R)
    );
endmodule
