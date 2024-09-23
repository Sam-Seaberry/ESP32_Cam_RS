ESP32 Cam 

This project sets up and streams video from a 
esp32 cam board via a http server hosted on 
the esp. Project is written in rust. To 
build first follow steps here:
https://docs.esp-rs.org/book/installation/index.html

Video stream has issues due to the esp wifi chip/antenna.
To improve stream qualify either lower JPEG resolution/FPS
or attach a better wifi antenna and apply a cooling solution
to the chip.
