"""
This module is used to fake the original hyperion functions and to store
interthread variables for led data and images

Created on 27.11.2014
Last updated on 13.3.2016

@author: Fabian Hertwig
@author: Juha Rantanen
"""

import threading

_lock = threading.Lock()

ledCount = 0

# the data as set in the hypercon application
# horizontal = 0
# vertical = 0
# first_led_offset = 0
# clockwise_direction = False
# corner_leds = False
leds = None
leds_top = None
leds_right = None
leds_bottom = None
leds_left = None
clockwise = False

# the dictionary the hyperion effect will access
args = {}

_ledData = None
_imageData = None
_imageWidth = 0
_imageHeight = 0
_abort = False

""" helper functions """

def init(_leds, _leds_top, _leds_right, _leds_bottom, _leds_left):
    """
    Initialise the fake hyperion.
    """
    global ledCount, leds, leds_top, leds_right, leds_bottom, leds_left
    global _ledData, _imageData, _imageWidth, _imageHeight

    ledCount = len(_leds)
    leds = _leds
    leds_top = _leds_top
    leds_right = _leds_right
    leds_bottom = _leds_bottom
    leds_left = _leds_left

    _imageWidth = len(leds_top) + 2
    _imageHeight = len(leds_left)
    _imageData = bytearray()

    for i in range(_imageWidth * _imageHeight * 3):
        _imageData.append(0)

    _ledData = bytearray()
    for x in range(ledCount * 3):
        _ledData.append(0)


def set_abort(abort_hyperion):
    global _abort
    _abort = abort_hyperion


def get_led_data():
    led_data_copy = bytearray()
    if _ledData:
        _lock.acquire()
        led_data_copy = bytearray(_ledData)
        _lock.release()
    return led_data_copy

def get_image_data():
    img_data_copy = bytearray()
    if _imageData:
        _lock.acquire()
        img_data_copy = bytearray(_imageData)
        _lock.release()
    return (_imageWidth, _imageHeight, img_data_copy)

def set_args(_args):
    global args
    args = _args


""" fake hyperion functions """

def abort():
    return _abort

def set_color_led_data(led_data):
    global _ledData
    _lock.acquire()
    _ledData = bytearray(led_data)
    _lock.release()


def set_color_rgb(red, green, blue):
    global _ledData
    _lock.acquire()
    for i in range(len(_ledData) // 3):
        _ledData[3*i] = red
        _ledData[3*i + 1] = green
        _ledData[3*i + 2] = blue
    _lock.release()

def setColor(*args):
    if len(args) == 1:
        set_color_led_data(args[0])
    elif len(args) == 3:
        set_color_rgb(args[0], args[1], args[2])
    else:
        raise TypeError('setColor takes 1 or 3 arguments')

def setImage(width, height, image_data):
    global _imageData, _imageWidth, _imageHeight
    _lock.acquire()
    _imageWidth = width
    _imageHeight = height
    _imageData = bytearray(image_data)
    _lock.release()
