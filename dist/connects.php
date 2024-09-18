<?php

$conn=mysqli_connect('localhost','root','','dress_customization');
if(!$conn){
    die("connection failed:".mysqli_connect_error($conn));
}
?>           