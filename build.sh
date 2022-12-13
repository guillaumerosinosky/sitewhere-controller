IMAGE=${DOCKER_IMAGE:-grosinosky/iot-simulator}
docker build . -t $IMAGE
docker push $IMAGE