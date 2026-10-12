"""
This module is used to display the leds in a gui

Created on 27.11.2014
Last updated on 13.3.2016

@author: Fabian Hertwig
@authir: Juha Rantanen
"""
from tkinter import Tk, Canvas, mainloop
import tkinter.font as tkFont
from app import hyperion

_led_height = 16
_led_width = 16

master = Tk()


def rgb_to_string(r, g, b):
    # As defined by tkinter documentation
    return "#%02x%02x%02x" % (r, g, b)


def createWindow(show_idx):
    # the empty window
    window_width = 1920//2-100
    window_height = 1080//2-100

    canvas = Canvas(master, width=window_width, height=window_height)
    canvas.pack()

    # list for the led representing rectangles
    gui_leds = []

    gui_font = tkFont.Font(family='Helvetica',size=8)

    for i, led in enumerate(hyperion.leds):

        x = round((led['x'] / 100.0) * (window_width-30))
        y = round((led['y'] / 100.0) * (window_height-30))

        # if i >= 17 and i <= 27:
        #     print 'Led {}: {} {}'.format(i, x, y)

        rect = canvas.create_rectangle(x, y, x + _led_width, y + _led_height, fill="black", outline="white")
        if show_idx:
            canvas.create_text(x + _led_width//2, y + _led_height//2, font=gui_font, fill="white", text=str(i))

        # leds_without_offset.append(rect)
        gui_leds.append(rect)

    # Call master which recalls itself to update the gui in the mainloop
    master.after(33, update_leds, canvas, gui_leds)
    mainloop()


def update_leds(canvas, gui_leds):
    for i, led in enumerate(gui_leds):
        change_color(canvas, led, i)

    master.after(33, update_leds, canvas, gui_leds)


def change_color(canvas, rect, led_number):
    led_data_copy = hyperion.get_led_data()
    if len(led_data_copy) >= 3 * led_number + 2:
        r = hyperion.get_led_data()[3 * led_number + 0]
        g = hyperion.get_led_data()[3 * led_number + 1]
        b = hyperion.get_led_data()[3 * led_number + 2]

        canvas.itemconfigure(rect, fill=rgb_to_string(r, g, b))